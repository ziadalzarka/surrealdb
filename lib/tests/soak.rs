mod helpers;
use peak_alloc::PeakAlloc;
use serial_test::serial;
use std::time::SystemTime;
use surrealdb::dbs::Session;
use surrealdb::err::Error;
use surrealdb::kvs::Datastore;
use test_log::test;

#[global_allocator]
static PEAK_ALLOC: PeakAlloc = PeakAlloc;

async fn soak_test(
	dbs: &Datastore,
	num_create: usize,
	num_iteration: usize,
	with_statements: bool,
) -> Result<(), Error> {
	let ses = Session::owner().with_ns("test").with_db("test");
	// We initially set a collection of records
	// After that, the test will not anymore add data to the database
	for i in 0..num_create {
		let sql = format!(
			"CREATE record:{} SET value={};",
			i,
			SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()
		);
		execute_sql(dbs, &ses, &sql, 1).await?;
	}
	let mut last_time = SystemTime::now();
	// We collect the initial memory usage
	let initial_mem = PEAK_ALLOC.current_usage_as_kb() as f64;
	{
		// Now we can begin the iteration and perform operations that do not add records to the database
		for i in 0..num_iteration {
			if with_statements {
				{
					let sql = format!("SELECT * FROM record PARALLEL;");
					execute_sql(dbs, &ses, &sql, 1).await?;
				}
				if num_create > 0 {
					let r = i % num_create;
					{
						let sql = format!("DELETE record:{}", r,);
						execute_sql(dbs, &ses, &sql, 1).await?;
					}
					{
						let sql = format!(
							"CREATE record:{} SET value={};",
							r,
							last_time.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()
						);
						execute_sql(dbs, &ses, &sql, 1).await?;
					}
					{
						let sql = format!(
							"UPDATE record:{} SET value={};",
							r,
							last_time.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()
						);
						execute_sql(dbs, &ses, &sql, 1).await?;
					}
				}
				{
					let sql = format!("SELECT COUNT() FROM record PARALLEL;");
					execute_sql(dbs, &ses, &sql, 1).await?;
				}
			}
			// We want to be sure the compiler does not bypass the loop when `with_select` is set to false
			last_time = SystemTime::now();
		}
	}
	// The test is done, let's collect the final memory usage and the growth ratio
	let final_mem = PEAK_ALLOC.current_usage_as_kb() as f64;
	let ratio = final_mem / initial_mem;
	println!(
		"Ratio: {} - InitialMEM: {} - FinalMEM: {} - LastTime: {:?}",
		ratio,
		initial_mem,
		final_mem,
		last_time.duration_since(SystemTime::UNIX_EPOCH).unwrap()
	);
	// We tolerate a ratio of 1%
	assert!(ratio <= 1.01, "Ratio: {}!", ratio);
	println!("OK\n");
	Ok(())
}

#[cfg(feature = "kv-mem")]
#[test(tokio::test)]
#[serial]
// This test is a dry test, no statements are run against the datastore.
// The role of this tests is to qualify the memory detection.
async fn soak_test_memory_dry() -> Result<(), Error> {
	let ds = helpers::new_ds().await?;
	soak_test(&ds, 0, 1000, false).await
}

#[cfg(feature = "kv-mem")]
#[test(tokio::test)]
#[serial]
async fn soak_test_memory_create_0_select_1000() -> Result<(), Error> {
	let ds = helpers::new_ds().await?;
	soak_test(&ds, 0, 1000, true).await
}

#[cfg(feature = "kv-mem")]
#[test(tokio::test)]
#[serial]
async fn soak_test_memory_create_1000_select_1000() -> Result<(), Error> {
	let ds = helpers::new_ds().await?;
	soak_test(&ds, 1000, 1000, true).await
}
#[cfg(feature = "kv-rocksdb")]
#[test(tokio::test)]
#[serial]
async fn soak_test_rocksdb_create_0_select_1000() -> Result<(), Error> {
	let ds = helpers::new_ds_rocksdb().await?;
	soak_test(&ds, 0, 1000, true).await
}

#[cfg(feature = "kv-rocksdb")]
#[test(tokio::test)]
#[serial]
async fn soak_test_rocksdb_create_1000_select_1000() -> Result<(), Error> {
	let ds = helpers::new_ds_rocksdb().await?;
	soak_test(&ds, 1000, 1000, true).await
}

// Helper
async fn execute_sql(
	dbs: &Datastore,
	ses: &Session,
	sql: &str,
	expected_result: usize,
) -> Result<(), Error> {
	let res = dbs.execute(&sql, &ses, None).await?;
	assert_eq!(res.len(), expected_result);
	for r in res {
		r.result?;
	}
	Ok(())
}
