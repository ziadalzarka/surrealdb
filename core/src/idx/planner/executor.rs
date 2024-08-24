use crate::ctx::Context;
use crate::dbs::{Options, Transaction};
use crate::doc::CursorDoc;
use crate::err::Error;
use crate::idx::docids::{DocId, DocIds};
use crate::idx::ft::analyzer::{Analyzer, TermsList, TermsSet};
use crate::idx::ft::scorer::BM25Scorer;
use crate::idx::ft::termdocs::TermsDocs;
use crate::idx::ft::terms::Terms;
use crate::idx::ft::{FtIndex, MatchRef};
use crate::idx::planner::iterators::{
	DocIdsIterator, IndexEqualThingIterator, IndexJoinThingIterator, IndexRangeThingIterator,
	IndexUnionThingIterator, MatchesThingIterator, ThingIterator, ThingsIterator,
	UniqueEqualThingIterator, UniqueJoinThingIterator, UniqueRangeThingIterator,
	UniqueUnionThingIterator,
};
use crate::idx::planner::knn::{KnnBruteForceResult, KnnPriorityList};
use crate::idx::planner::plan::IndexOperator::Matches;
use crate::idx::planner::plan::{IndexOperator, IndexOption, RangeValue};
use crate::idx::planner::tree::{IdiomPosition, IndexRef, IndexesMap};
use crate::idx::planner::IterationStage;
use crate::idx::trees::mtree::MTreeIndex;
use crate::idx::trees::store::hnsw::SharedHnswIndex;
use crate::idx::IndexKeyBase;
use crate::kvs;
use crate::kvs::{Key, TransactionType};
use crate::sql::index::{Distance, Index};
use crate::sql::statements::DefineIndexStatement;
use crate::sql::{Array, Expression, Idiom, Number, Object, Table, Thing, Value};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;
use async_recursion::async_recursion;

pub(super) type KnnBruteForceEntry = (KnnPriorityList, Idiom, Arc<Vec<Number>>, Distance);

pub(super) struct KnnBruteForceExpression {
	k: u32,
	id: Idiom,
	obj: Arc<Vec<Number>>,
	d: Distance,
}

impl KnnBruteForceExpression {
	pub(super) fn new(k: u32, id: Idiom, obj: Arc<Vec<Number>>, d: Distance) -> Self {
		Self {
			k,
			id,
			obj,
			d,
		}
	}
}

pub(super) type KnnBruteForceExpressions = HashMap<Arc<Expression>, KnnBruteForceExpression>;

pub(super) type KnnExpressions = HashSet<Arc<Expression>>;

#[derive(Clone)]
pub(crate) struct QueryExecutor(Arc<InnerQueryExecutor>);

pub(super) struct InnerQueryExecutor {
	table: String,
	ft_map: HashMap<IndexRef, FtIndex>,
	mr_entries: HashMap<MatchRef, FtEntry>,
	exp_entries: HashMap<Arc<Expression>, FtEntry>,
	it_entries: Vec<IteratorEntry>,
	index_definitions: Vec<DefineIndexStatement>,
	mt_entries: HashMap<Arc<Expression>, MtEntry>,
	hnsw_entries: HashMap<Arc<Expression>, HnswEntry>,
	knn_bruteforce_entries: HashMap<Arc<Expression>, KnnBruteForceEntry>,
}

impl From<InnerQueryExecutor> for QueryExecutor {
	fn from(value: InnerQueryExecutor) -> Self {
		Self(Arc::new(value))
	}
}

pub(crate) type IteratorRef = u16;

pub(super) enum IteratorEntry {
	Single(Arc<Expression>, IndexOption),
	Range(HashSet<Arc<Expression>>, IndexRef, RangeValue, RangeValue),
}

impl IteratorEntry {
	pub(super) fn explain(&self, ix_def: &[DefineIndexStatement]) -> Value {
		match self {
			Self::Single(_, io) => io.explain(ix_def),
			Self::Range(_, ir, from, to) => {
				let mut e = HashMap::default();
				if let Some(ix) = ix_def.get(*ir as usize) {
					e.insert("index", Value::from(ix.name.0.to_owned()));
				}
				e.insert("from", Value::from(from));
				e.insert("to", Value::from(to));
				Value::from(Object::from(e))
			}
		}
	}
}
impl InnerQueryExecutor {
	#[allow(clippy::too_many_arguments)]
	pub(super) async fn new(
		ctx: &Context<'_>,
		opt: &Options,
		txn: &Transaction,
		table: &Table,
		im: IndexesMap,
		knns: KnnExpressions,
		kbtes: KnnBruteForceExpressions,
	) -> Result<Self, Error> {
		let mut mr_entries = HashMap::default();
		let mut exp_entries = HashMap::default();
		let mut ft_map = HashMap::default();
		let mut mt_map: HashMap<IndexRef, MTreeIndex> = HashMap::default();
		let mut mt_entries = HashMap::default();
		let mut hnsw_map: HashMap<IndexRef, SharedHnswIndex> = HashMap::default();
		let mut hnsw_entries = HashMap::default();
		let mut knn_bruteforce_entries = HashMap::with_capacity(knns.len());

		// Create all the instances of FtIndex
		// Build the FtEntries and map them to Idioms and MatchRef
		for (exp, io) in im.options {
			let ix_ref = io.ix_ref();
			if let Some(idx_def) = im.definitions.get(ix_ref as usize) {
				match &idx_def.index {
					Index::Search(p) => {
						let ft_entry = match ft_map.entry(ix_ref) {
							Entry::Occupied(e) => FtEntry::new(ctx, opt, txn, e.get(), io).await?,
							Entry::Vacant(e) => {
								let ikb = IndexKeyBase::new(opt, idx_def);
								let ft = FtIndex::new(
									ctx.get_index_stores(),
									opt,
									txn,
									p.az.as_str(),
									ikb,
									p,
									TransactionType::Read,
								)
								.await?;
								let fte = FtEntry::new(ctx, opt, txn, &ft, io).await?;
								e.insert(ft);
								fte
							}
						};
						if let Some(e) = ft_entry {
							if let Matches(_, Some(mr)) = e.0.index_option.op() {
								if mr_entries.insert(*mr, e.clone()).is_some() {
									return Err(Error::DuplicatedMatchRef {
										mr: *mr,
									});
								}
							}
							exp_entries.insert(exp, e);
						}
					}
					Index::MTree(p) => {
						if let IndexOperator::Knn(a, k) = io.op() {
							let mut tx = txn.lock().await;
							let entry = match mt_map.entry(ix_ref) {
								Entry::Occupied(e) => MtEntry::new(&mut tx, e.get(), a, *k).await?,
								Entry::Vacant(e) => {
									let ikb = IndexKeyBase::new(opt, idx_def);
									let mt = MTreeIndex::new(
										ctx.get_index_stores(),
										&mut tx,
										ikb,
										p,
										TransactionType::Read,
									)
									.await?;
									let entry = MtEntry::new(&mut tx, &mt, a, *k).await?;
									e.insert(mt);
									entry
								}
							};
							mt_entries.insert(exp, entry);
						}
					}
					Index::Hnsw(p) => {
						if let IndexOperator::Ann(a, n, ef) = io.op() {
							let entry = match hnsw_map.entry(ix_ref) {
								Entry::Occupied(e) => {
									HnswEntry::new(e.get().clone(), a, *n, *ef).await?
								}
								Entry::Vacant(e) => {
									let hnsw = ctx
										.get_index_stores()
										.get_index_hnsw(opt, idx_def, p)
										.await;
									let entry = HnswEntry::new(hnsw.clone(), a, *n, *ef).await?;
									e.insert(hnsw);
									entry
								}
							};
							hnsw_entries.insert(exp, entry);
						}
					}
					_ => {}
				}
			}
		}

		for (exp, knn) in kbtes {
			knn_bruteforce_entries
				.insert(exp, (KnnPriorityList::new(knn.k as usize), knn.id, knn.obj, knn.d));
		}

		Ok(Self {
			table: table.0.clone(),
			ft_map,
			mr_entries,
			exp_entries,
			it_entries: Vec::new(),
			index_definitions: im.definitions,
			mt_entries,
			hnsw_entries,
			knn_bruteforce_entries,
		})
	}

	pub(super) fn add_iterator(&mut self, it_entry: IteratorEntry) -> IteratorRef {
		let ir = self.it_entries.len();
		self.it_entries.push(it_entry);
		ir as IteratorRef
	}
}

impl QueryExecutor {
	pub(crate) async fn knn(
		&self,
		ctx: &Context<'_>,
		opt: &Options,
		txn: &Transaction,
		thg: &Thing,
		doc: Option<&CursorDoc<'_>>,
		exp: &Expression,
	) -> Result<Value, Error> {
		if let Some(IterationStage::Iterate(e)) = ctx.get_iteration_stage() {
			if let Some(results) = e {
				return Ok(results.contains(exp, thg).into());
			}
			Ok(Value::Bool(false))
		} else {
			if let Some((p, id, val, dist)) = self.0.knn_bruteforce_entries.get(exp) {
				let v: Vec<Number> = id.compute(ctx, opt, txn, doc).await?.try_into()?;
				let dist = dist.compute(&v, val.as_ref())?;
				p.add(dist, thg).await;
			}
			Ok(Value::Bool(true))
		}
	}

	pub(super) async fn build_bruteforce_knn_result(&self) -> KnnBruteForceResult {
		let mut result = KnnBruteForceResult::with_capacity(self.0.knn_bruteforce_entries.len());
		for (e, (p, _, _, _)) in &self.0.knn_bruteforce_entries {
			result.insert(e.clone(), p.build().await);
		}
		result
	}

	pub(crate) fn is_table(&self, tb: &str) -> bool {
		self.0.table.eq(tb)
	}

	pub(crate) fn has_bruteforce_knn(&self) -> bool {
		!self.0.knn_bruteforce_entries.is_empty()
	}

	/// Returns `true` if the expression is matching the current iterator.
	pub(crate) fn is_iterator_expression(&self, ir: IteratorRef, exp: &Expression) -> bool {
		match self.0.it_entries.get(ir as usize) {
			Some(IteratorEntry::Single(e, ..)) => exp.eq(e.as_ref()),
			Some(IteratorEntry::Range(es, ..)) => es.contains(exp),
			_ => false,
		}
	}

	pub(crate) fn explain(&self, itr: IteratorRef) -> Value {
		match self.0.it_entries.get(itr as usize) {
			Some(ie) => ie.explain(self.0.index_definitions.as_slice()),
			None => Value::None,
		}
	}

	fn get_match_ref(match_ref: &Value) -> Option<MatchRef> {
		if let Value::Number(n) = match_ref {
			let m = n.to_int() as u8;
			Some(m)
		} else {
			None
		}
	}

	pub(crate) async fn new_iterator(
		&self,
		opt: &Options,
		it_ref: IteratorRef,
	) -> Result<Option<ThingIterator>, Error> {
		if let Some(it_entry) = self.0.it_entries.get(it_ref as usize) {
			match it_entry {
				IteratorEntry::Single(_, io) => self.new_single_iterator(opt, it_ref, io).await,
				IteratorEntry::Range(_, ir, from, to) => {
					Ok(self.new_range_iterator(opt, *ir, from, to))
				}
			}
		} else {
			Ok(None)
		}
	}

	#[async_recursion]
	async fn new_single_iterator(
		&self,
		opt: &Options,
		it_ref: IteratorRef,
		io: &IndexOption,
	) -> Result<Option<ThingIterator>, Error> {
		if let Some(ix) = self.get_index_def(io.ix_ref()) {
			match ix.index {
				Index::Idx => Ok(self.new_index_iterator(opt, it_ref, ix, io.clone()).await?),
				Index::Uniq => {
					Ok(self.new_unique_index_iterator(opt, it_ref, ix, io.clone()).await?)
				}
				Index::Search {
					..
				} => self.new_search_index_iterator(it_ref, io.clone()).await,
				Index::MTree(_) => Ok(self.new_mtree_index_knn_iterator(it_ref)),
				Index::Hnsw(_) => Ok(self.new_hnsw_index_ann_iterator(it_ref)),
			}
		} else {
			Ok(None)
		}
	}

	#[async_recursion]
	async fn new_index_iterator(
		&self,
		opt: &Options,
		it_ref: IteratorRef,
		ix: &DefineIndexStatement,
		io: IndexOption,
	) -> Result<Option<ThingIterator>, Error> {
		Ok(match io.op() {
			IndexOperator::Equality(value) | IndexOperator::Exactness(value) => {
				Some(ThingIterator::IndexEqual(IndexEqualThingIterator::new(
					opt.ns(),
					opt.db(),
					&ix.what,
					&ix.name,
					value,
				)))
			}
			IndexOperator::Union(value) => Some(ThingIterator::IndexUnion(
				IndexUnionThingIterator::new(opt.ns(), opt.db(), &ix.what, &ix.name, value),
			)),
			IndexOperator::Join(ios) => {
				let iterators = self.build_iterators(opt, it_ref, ios).await?;
				let index_join = Box::new(IndexJoinThingIterator::new(opt, ix, iterators));
				Some(ThingIterator::IndexJoin(index_join))
			}
			_ => None,
		})
	}

	fn new_range_iterator(
		&self,
		opt: &Options,
		ir: IndexRef,
		from: &RangeValue,
		to: &RangeValue,
	) -> Option<ThingIterator> {
		if let Some(ix) = self.get_index_def(ir) {
			match ix.index {
				Index::Idx => {
					return Some(ThingIterator::IndexRange(IndexRangeThingIterator::new(
						opt.ns(),
						opt.db(),
						&ix.what,
						&ix.name,
						from,
						to,
					)))
				}
				Index::Uniq => {
					return Some(ThingIterator::UniqueRange(UniqueRangeThingIterator::new(
						opt.ns(),
						opt.db(),
						&ix.what,
						&ix.name,
						from,
						to,
					)))
				}
				_ => {}
			}
		}
		None
	}

	async fn new_unique_index_iterator(
		&self,
		opt: &Options,
		it_ref: IteratorRef,
		ix: &DefineIndexStatement,
		io: IndexOption,
	) -> Result<Option<ThingIterator>, Error> {
		Ok(match io.op() {
			IndexOperator::Equality(value) => Some(ThingIterator::UniqueEqual(
				UniqueEqualThingIterator::new(opt.ns(), opt.db(), &ix.what, &ix.name, value),
			)),
			IndexOperator::Union(value) => {
				Some(ThingIterator::UniqueUnion(UniqueUnionThingIterator::new(opt, ix, value)))
			}
			IndexOperator::Join(ios) => {
				let iterators = self.build_iterators(opt, it_ref, ios).await?;
				let unique_join = Box::new(UniqueJoinThingIterator::new(opt, ix, iterators));
				Some(ThingIterator::UniqueJoin(unique_join))
			}
			_ => None,
		})
	}

	async fn new_search_index_iterator(
		&self,
		it_ref: IteratorRef,
		io: IndexOption,
	) -> Result<Option<ThingIterator>, Error> {
		if let Some(IteratorEntry::Single(exp, ..)) = self.0.it_entries.get(it_ref as usize) {
			if let Matches(_, _) = io.op() {
				if let Some(fti) = self.0.ft_map.get(&io.ix_ref()) {
					if let Some(fte) = self.0.exp_entries.get(exp) {
						let it = MatchesThingIterator::new(fti, fte.0.terms_docs.clone()).await?;
						return Ok(Some(ThingIterator::Matches(it)));
					}
				}
			}
		}
		Ok(None)
	}

	fn new_mtree_index_knn_iterator(&self, it_ref: IteratorRef) -> Option<ThingIterator> {
		if let Some(IteratorEntry::Single(exp, ..)) = self.0.it_entries.get(it_ref as usize) {
			if let Some(mte) = self.0.mt_entries.get(exp) {
				let it = DocIdsIterator::new(
					mte.doc_ids.clone(),
					mte.res.iter().map(|(d, _)| *d).collect(),
				);
				return Some(ThingIterator::Knn(it));
			}
		}
		None
	}

	fn new_hnsw_index_ann_iterator(&self, it_ref: IteratorRef) -> Option<ThingIterator> {
		if let Some(IteratorEntry::Single(exp, ..)) = self.0.it_entries.get(it_ref as usize) {
			if let Some(he) = self.0.hnsw_entries.get(exp) {
				let it = ThingsIterator::new(he.res.iter().map(|(thg, _)| thg.clone()).collect());
				return Some(ThingIterator::Things(it));
			}
		}
		None
	}

	#[async_recursion]
	async fn build_iterators(
		&self,
		opt: &Options,
		it_ref: IteratorRef,
		ios: &[IndexOption],
	) -> Result<VecDeque<ThingIterator>, Error> {
		let mut iterators = VecDeque::with_capacity(ios.len());
		for io in ios {
			if let Some(it) = Box::pin(self.new_single_iterator(opt, it_ref, io)).await? {
				iterators.push_back(it);
			}
		}
		Ok(iterators)
	}

	fn get_index_def(&self, ir: IndexRef) -> Option<&DefineIndexStatement> {
		self.0.index_definitions.get(ir as usize)
	}

	#[allow(clippy::too_many_arguments)]
	pub(crate) async fn matches(
		&self,
		ctx: &Context<'_>,
		opt: &Options,
		txn: &Transaction,
		thg: &Thing,
		exp: &Expression,
		l: Value,
		r: Value,
	) -> Result<bool, Error> {
		if let Some(ft) = self.0.exp_entries.get(exp) {
			if let Some(ix_def) = self.get_index_def(ft.0.index_option.ix_ref()) {
				if self.0.table.eq(&ix_def.what.0) {
					return self.matches_with_doc_id(txn, thg, ft).await;
				}
			}
			return self.matches_with_value(ctx, opt, txn, ft, l, r).await;
		}

		// If no previous case were successful, we end up with a user error
		Err(Error::NoIndexFoundForMatch {
			value: exp.to_string(),
		})
	}

	async fn matches_with_doc_id(
		&self,
		txn: &Transaction,
		thg: &Thing,
		ft: &FtEntry,
	) -> Result<bool, Error> {
		let mut run = txn.lock().await;
		let doc_key: Key = thg.into();
		if let Some(doc_id) = ft.0.doc_ids.read().await.get_doc_id(&mut run, doc_key).await? {
			let term_goals = ft.0.terms_docs.len();
			// If there is no terms, it can't be a match
			if term_goals == 0 {
				return Ok(false);
			}
			for opt_td in ft.0.terms_docs.iter() {
				if let Some((_, docs)) = opt_td {
					if !docs.contains(doc_id) {
						return Ok(false);
					}
				} else {
					// If one of the term is missing, it can't be a match
					return Ok(false);
				}
			}
			return Ok(true);
		}
		Ok(false)
	}

	async fn matches_with_value(
		&self,
		ctx: &Context<'_>,
		opt: &Options,
		txn: &Transaction,
		ft: &FtEntry,
		l: Value,
		r: Value,
	) -> Result<bool, Error> {
		// If the query terms contains terms that are unknown in the index
		// of if there is not terms in the query
		// we are sure that it does not match any document
		if !ft.0.query_terms_set.is_matchable() {
			return Ok(false);
		}
		let v = match ft.0.index_option.id_pos() {
			IdiomPosition::Left => r,
			IdiomPosition::Right => l,
		};
		let terms = ft.0.terms.read().await;
		// Extract the terms set from the record
		let t = ft.0.analyzer.extract_indexing_terms(ctx, opt, txn, &terms, v).await?;
		Ok(ft.0.query_terms_set.is_subset(&t))
	}

	fn get_ft_entry(&self, match_ref: &Value) -> Option<&FtEntry> {
		if let Some(mr) = Self::get_match_ref(match_ref) {
			self.0.mr_entries.get(&mr)
		} else {
			None
		}
	}

	fn get_ft_entry_and_index(&self, match_ref: &Value) -> Option<(&FtEntry, &FtIndex)> {
		if let Some(e) = self.get_ft_entry(match_ref) {
			if let Some(ft) = self.0.ft_map.get(&e.0.index_option.ix_ref()) {
				return Some((e, ft));
			}
		}
		None
	}

	#[allow(clippy::too_many_arguments)]
	pub(crate) async fn highlight(
		&self,
		txn: &Transaction,
		thg: &Thing,
		prefix: Value,
		suffix: Value,
		match_ref: Value,
		partial: bool,
		doc: &Value,
	) -> Result<Value, Error> {
		if let Some((e, ft)) = self.get_ft_entry_and_index(&match_ref) {
			let mut run = txn.lock().await;
			return ft
				.highlight(
					&mut run,
					thg,
					&e.0.query_terms_list,
					prefix,
					suffix,
					partial,
					e.0.index_option.id_ref(),
					doc,
				)
				.await;
		}
		Ok(Value::None)
	}

	pub(crate) async fn offsets(
		&self,
		txn: &Transaction,
		thg: &Thing,
		match_ref: Value,
		partial: bool,
	) -> Result<Value, Error> {
		if let Some((e, ft)) = self.get_ft_entry_and_index(&match_ref) {
			let mut run = txn.lock().await;
			return ft.extract_offsets(&mut run, thg, &e.0.query_terms_list, partial).await;
		}
		Ok(Value::None)
	}

	pub(crate) async fn score(
		&self,
		txn: &Transaction,
		match_ref: &Value,
		rid: &Thing,
		mut doc_id: Option<DocId>,
	) -> Result<Value, Error> {
		if let Some(e) = self.get_ft_entry(match_ref) {
			if let Some(scorer) = &e.0.scorer {
				let mut run = txn.lock().await;
				if doc_id.is_none() {
					let key: Key = rid.into();
					doc_id = e.0.doc_ids.read().await.get_doc_id(&mut run, key).await?;
				};
				if let Some(doc_id) = doc_id {
					let score = scorer.score(&mut run, doc_id).await?;
					if let Some(score) = score {
						return Ok(Value::from(score));
					}
				}
			}
		}
		Ok(Value::None)
	}
}

#[derive(Clone)]
struct FtEntry(Arc<Inner>);

struct Inner {
	index_option: IndexOption,
	doc_ids: Arc<RwLock<DocIds>>,
	analyzer: Arc<Analyzer>,
	query_terms_set: TermsSet,
	query_terms_list: TermsList,
	terms: Arc<RwLock<Terms>>,
	terms_docs: TermsDocs,
	scorer: Option<BM25Scorer>,
}

impl FtEntry {
	async fn new(
		ctx: &Context<'_>,
		opt: &Options,
		txn: &Transaction,
		ft: &FtIndex,
		io: IndexOption,
	) -> Result<Option<Self>, Error> {
		if let Matches(qs, _) = io.op() {
			let (terms_list, terms_set) =
				ft.extract_querying_terms(ctx, opt, txn, qs.to_owned()).await?;
			let mut tx = txn.lock().await;
			let terms_docs = Arc::new(ft.get_terms_docs(&mut tx, &terms_list).await?);
			Ok(Some(Self(Arc::new(Inner {
				index_option: io,
				doc_ids: ft.doc_ids(),
				analyzer: ft.analyzer(),
				query_terms_set: terms_set,
				query_terms_list: terms_list,
				scorer: ft.new_scorer(terms_docs.clone())?,
				terms: ft.terms(),
				terms_docs,
			}))))
		} else {
			Ok(None)
		}
	}
}

#[derive(Clone)]
pub(super) struct MtEntry {
	doc_ids: Arc<RwLock<DocIds>>,
	res: VecDeque<(DocId, f64)>,
}

impl MtEntry {
	async fn new(
		tx: &mut kvs::Transaction,
		mt: &MTreeIndex,
		a: &Array,
		k: u32,
	) -> Result<Self, Error> {
		let res = mt.knn_search(tx, a, k as usize).await?;
		Ok(Self {
			res,
			doc_ids: mt.doc_ids(),
		})
	}
}

#[derive(Clone)]
pub(super) struct HnswEntry {
	res: VecDeque<(Thing, f64)>,
}

impl HnswEntry {
	async fn new(h: SharedHnswIndex, a: &Array, n: usize, ef: usize) -> Result<Self, Error> {
		let res = h.read().await.knn_search(a, n, ef)?;
		Ok(Self {
			res,
		})
	}
}
