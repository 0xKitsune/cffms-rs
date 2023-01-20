use core::panic;
use std::{
    fs::read_to_string,
    panic::resume_unwind,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use ethers::{
    providers::Middleware,
    types::{H160, U256},
};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use serde_json::{Map, Value};

use crate::{
    dex::{Dex, DexVariant},
    error::CFMMError,
    pool::{Pool, UniswapV2Pool, UniswapV3Pool},
    throttle::RequestThrottle,
};

//Get all pairs and sync reserve values for each Dex in the `dexes` vec.
pub async fn sync_pairs_from_checkpoint<M: 'static + Middleware>(
    path_to_checkpoint: String,
    middleware: Arc<M>,
) -> Result<(Vec<Dex>, Vec<Pool>), CFMMError<M>> {
    sync_pairs_from_checkpoint_with_throttle(path_to_checkpoint, middleware, 0).await
}

//Get all pairs from last synced block and sync reserve values for each Dex in the `dexes` vec.
pub async fn sync_pairs_from_checkpoint_with_throttle<M: 'static + Middleware>(
    path_to_checkpoint: String,
    middleware: Arc<M>,
    requests_per_second_limit: usize,
) -> Result<(Vec<Dex>, Vec<Pool>), CFMMError<M>> {
    let request_throttle = Arc::new(Mutex::new(RequestThrottle::new(requests_per_second_limit)));
    //Initialize multi progress bar
    let multi_progress_bar = MultiProgress::new();
    let _progress_bar = multi_progress_bar.add(ProgressBar::new(0));

    //Read in checkpoint
    let (dexes, mut pools) = deconstruct_checkpoint(path_to_checkpoint);

    //TODO: set progress bar length and style

    //Update reserves for all pools
    for pool in pools.iter_mut() {
        request_throttle.lock().unwrap().increment_or_sleep(2);
        pool.sync_pool(middleware.clone()).await?;
    }

    Ok((dexes, pools))
}

//Get all pairs and sync reserve values for each Dex in the `dexes` vec.
pub async fn generate_checkpoint<M: 'static + Middleware>(
    dexes: Vec<Dex>,
    middleware: Arc<M>,
    checkpoint_file_name: String,
) -> Result<(), CFMMError<M>> {
    //Sync pairs with throttle but set the requests per second limit to 0, disabling the throttle.
    generate_checkpoint_with_throttle(dexes, middleware, 0, checkpoint_file_name).await
}

//Get all pairs and sync reserve values for each Dex in the `dexes` vec.
pub async fn generate_checkpoint_with_throttle<M: 'static + Middleware>(
    dexes: Vec<Dex>,
    middleware: Arc<M>,
    requests_per_second_limit: usize,
    checkpoint_file_name: String,
) -> Result<(), CFMMError<M>> {
    //Initialize a new request throttle
    let request_throttle = Arc::new(Mutex::new(RequestThrottle::new(requests_per_second_limit)));

    //Aggregate the populated pools from each thread
    let mut aggregated_pools: Vec<Pool> = vec![];
    let mut handles = vec![];

    //Initialize multi progress bar
    let multi_progress_bar = MultiProgress::new();

    //For each dex supplied, get all pair created events and get reserve values
    for dex in dexes.clone() {
        let async_provider = middleware.clone();
        let request_throttle = request_throttle.clone();
        let progress_bar = multi_progress_bar.add(ProgressBar::new(0));

        handles.push(tokio::spawn(async move {
            progress_bar.set_style(
                ProgressStyle::with_template("{msg} {bar:40.cyan/blue} {pos:>7}/{len:7} Blocks")
                    .unwrap()
                    .progress_chars("##-"),
            );

            let mut pools = dex
                .get_all_pools(
                    request_throttle.clone(),
                    progress_bar.clone(),
                    async_provider.clone(),
                )
                .await?;

            progress_bar.reset();
            progress_bar.set_style(
                ProgressStyle::with_template("{msg} {bar:40.cyan/blue} {pos:>7}/{len:7} Pairs")
                    .unwrap()
                    .progress_chars("##-"),
            );

            dex.get_all_pool_data(
                &mut pools,
                request_throttle.clone(),
                progress_bar.clone(),
                async_provider.clone(),
            )
            .await?;

            progress_bar.finish_and_clear();
            progress_bar.set_message(format!(
                "Finished syncing pools for {} ✅",
                dex.factory_address()
            ));

            progress_bar.finish();

            Ok::<_, CFMMError<M>>(pools)
        }));
    }

    for handle in handles {
        match handle.await {
            Ok(sync_result) => aggregated_pools.extend(sync_result?),
            Err(err) => {
                {
                    if err.is_panic() {
                        // Resume the panic on the main task
                        resume_unwind(err.into_panic());
                    }
                }
            }
        }
    }

    let latest_block = middleware
        .get_block_number()
        .await
        .map_err(CFMMError::MiddlewareError)?;

    println!("total pools :{}", aggregated_pools.len());

    construct_checkpoint(
        dexes,
        &aggregated_pools,
        latest_block.as_u64(),
        checkpoint_file_name,
    );

    Ok(())
}

//Syncs all reserve values for pools in checkpoint and returns a vec of Pool
pub async fn sync_pools_from_checkpoint<M: Middleware>(
    path_to_checkpoint: String,
    middleware: Arc<M>,
) -> Result<(Vec<Dex>, Vec<Pool>), CFMMError<M>> {
    sync_pools_from_checkpoint_with_throttle(path_to_checkpoint, middleware, 0).await
}

//Syncs all reserve values with throttle for pools in checkpoint and returns a vec of Pool
pub async fn sync_pools_from_checkpoint_with_throttle<M: Middleware>(
    path_to_checkpoint: String,
    middleware: Arc<M>,
    requests_per_second_limit: usize,
) -> Result<(Vec<Dex>, Vec<Pool>), CFMMError<M>> {
    let request_throttle = Arc::new(Mutex::new(RequestThrottle::new(requests_per_second_limit)));

    let multi_progress_bar = MultiProgress::new();
    let progress_bar = multi_progress_bar.add(ProgressBar::new(0));

    progress_bar.set_style(
        ProgressStyle::with_template("{msg} {bar:40.cyan/blue} {pos:>7}/{len:7} Pairs")
            .unwrap()
            .progress_chars("##-"),
    );

    //Read in checkpoint
    let (dexes, mut pools) = deconstruct_checkpoint(path_to_checkpoint);

    progress_bar.set_length(pools.len() as u64);
    progress_bar.set_message("Syncing reserves");

    //Update reserves for all pools
    for pool in pools.iter_mut() {
        request_throttle.lock().unwrap().increment_or_sleep(2);
        progress_bar.inc(1);

        match pool.sync_pool(middleware.clone()).await {
            Ok(_) => {}
            Err(pair_sync_error) => match pair_sync_error {
                CFMMError::MiddlewareError(middleware_error) => {
                    return Err(CFMMError::MiddlewareError(middleware_error))
                }
                _ => continue,
            },
        };
    }

    Ok((dexes, pools))
}

pub fn deconstruct_checkpoint(path_to_checkpoint: String) -> (Vec<Dex>, Vec<Pool>) {
    let mut dexes = vec![];

    let checkpoint_json: serde_json::Value = serde_json::from_str(
        read_to_string(path_to_checkpoint)
            .expect("Error when reading in checkpoint json")
            .as_str(),
    )
    .expect("Error when converting checkpoint file contents to serde_json::Value");

    for dex_data in checkpoint_json
        .get("dexes")
        .expect("Could not get checkpoint_data")
        .as_array()
        .expect("Could not unwrap checkpoint json into array")
        .iter()
    {
        let dex = deconstruct_dex_from_checkpoint(
            dex_data
                .as_object()
                .expect("Dex checkpoint is not formatted correctly"),
        );

        dexes.push(dex);
    }

    //get all pools
    let pools_array = checkpoint_json
        .get("pools")
        .expect("Could not get pools from checkpoint")
        .as_array()
        .expect("Could not convert pools to value array");

    let pools = deconstruct_pools_from_checkpoint(pools_array);

    (dexes, pools)
}

pub fn deconstruct_dex_from_checkpoint(dex_map: &Map<String, Value>) -> Dex {
    let dex_variant = match dex_map
        .get("dex_variant")
        .expect("Checkpoint formatted incorrectly, could not get dex_variant.")
        .as_str()
        .expect("Could not convert dex variant to string")
        .to_lowercase()
        .as_str()
    {
        "uniswapv2" => DexVariant::UniswapV2,
        "uniswapv3" => DexVariant::UniswapV3,
        other => {
            panic!("Unrecognized dex variant in checkpoint: {:?}", other)
        }
    };

    let latest_synced_block = dex_map
        .get("latest_synced_block")
        .expect("Checkpoint formatted incorrectly, could not get dex latest_synced_block.")
        .as_u64()
        .expect("Could not convert latest_synced_block to u64");

    let factory_address = H160::from_str(
        dex_map
            .get("factory_address")
            .expect("Checkpoint formatted incorrectly, could not get dex factory_address.")
            .as_str()
            .expect("Could not convert factory_address to str"),
    )
    .expect("Could not convert checkpoint factory_address to H160.");

    Dex::new(factory_address, dex_variant, latest_synced_block)
}

pub fn deconstruct_pools_from_checkpoint(pools_array: &Vec<Value>) -> Vec<Pool> {
    let mut pools = vec![];

    for pool_value in pools_array {
        let pool_map = pool_value
            .as_object()
            .expect("Could not convert pool value to map");

        let pool_dex_variant = match pool_map
            .get("dex_variant")
            .expect("Could not get pool dex_variant")
            .as_str()
            .expect("Could not convert dex_variant to str")
            .to_lowercase()
            .as_str()
        {
            "uniswapv2" => DexVariant::UniswapV2,
            "uniswapv3" => DexVariant::UniswapV3,
            _ => {
                panic!("Unrecognized pool dex variant")
            }
        };

        match pool_dex_variant {
            DexVariant::UniswapV2 | DexVariant::UniswapV3 => {
                let addr = H160::from_str(
                    pool_map
                        .get("address")
                        .unwrap_or_else(|| panic!("Could not get pool address {:?}", pool_map))
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("Could not convert pool address to str {:?}", pool_map)
                        }),
                )
                .expect("Could not convert token_a to H160");

                let token_a = H160::from_str(
                    pool_map
                        .get("token_a")
                        .unwrap_or_else(|| panic!("Could not get token_a {:?}", pool_map))
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("Could not convert token_a to str {:?}", pool_map)
                        }),
                )
                .expect("Could not convert token_a to H160");

                let token_a_decimals = pool_map
                    .get("token_a_decimals")
                    .unwrap_or_else(|| panic!("Could not get token_a_decimals {:?}", pool_map))
                    .as_u64()
                    .expect("Could not convert token_a_decimals to u64")
                    as u8;

                let token_b = H160::from_str(
                    pool_map
                        .get("token_b")
                        .unwrap_or_else(|| panic!("Could not get token_b {:?}", pool_map))
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("Could not convert token_b to str {:?}", pool_map)
                        }),
                )
                .expect("Could not convert token_b to H160");

                let token_b_decimals = pool_map
                    .get("token_b_decimals")
                    .unwrap_or_else(|| panic!("Could not get token_b_decimals {:?}", pool_map))
                    .as_u64()
                    .expect("Could not convert token_b_decimals to u64")
                    as u8;

                let _a_to_b = pool_map
                    .get("a_to_b")
                    .unwrap_or_else(|| panic!("Could not get a_to_b {:?}", pool_map))
                    .as_bool()
                    .expect("Could not convert a_to_b to bool");

                let fee = pool_map
                    .get("fee")
                    .unwrap_or_else(|| panic!("Could not get fee {:?}", pool_map))
                    .as_u64()
                    .expect("Could not convert fee to u64") as u32;

                match pool_dex_variant {
                    DexVariant::UniswapV2 => {
                        pools.push(Pool::UniswapV2(UniswapV2Pool::new(
                            addr,
                            token_a,
                            token_a_decimals,
                            token_b,
                            token_b_decimals,
                            0,
                            0,
                            fee,
                        )));
                    }

                    DexVariant::UniswapV3 => {
                        pools.push(Pool::UniswapV3(UniswapV3Pool::new(
                            addr,
                            token_a,
                            token_a_decimals,
                            token_b,
                            token_b_decimals,
                            fee,
                            0,
                            U256::zero(),
                            0,
                            0,
                            0,
                        )));
                    }
                }
            }
        }
    }

    pools
}

pub fn construct_checkpoint(
    dexes: Vec<Dex>,
    pools: &Vec<Pool>,
    latest_block: u64,
    checkpoint_file_name: String,
) {
    let mut checkpoint = Map::new();

    //Insert checkpoint_timestamp
    let checkpoint_timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f32() as u32;

    checkpoint.insert(
        String::from("checkpoint_timestamp"),
        checkpoint_timestamp.into(),
    );

    //Add dexes to checkpoint
    let mut dexes_array: Vec<Value> = vec![];
    for dex in dexes {
        let mut dex_map = Map::new();

        dex_map.insert(String::from("latest_synced_block"), latest_block.into());

        dex_map.insert(
            String::from("factory_address"),
            format!("{:?}", dex.factory_address()).into(),
        );

        match dex {
            Dex::UniswapV2(_) => {
                dex_map.insert(
                    String::from("dex_variant"),
                    String::from("UniswapV2").into(),
                );
            }

            Dex::UniswapV3(_) => {
                dex_map.insert(
                    String::from("dex_variant"),
                    String::from("UniswapV3").into(),
                );
            }
        }

        dexes_array.push(Value::Object(dex_map));
    }

    checkpoint.insert(String::from("dexes"), dexes_array.into());

    //Insert pools into checkpoint
    let mut pools_array: Vec<Value> = vec![];
    for pool in pools {
        let mut pool_map = Map::new();

        match pool {
            Pool::UniswapV2(uniswap_v2_pool) => {
                pool_map.insert(
                    String::from("dex_variant"),
                    String::from("UniswapV2").into(),
                );

                pool_map.insert(
                    String::from("address"),
                    format!("{:?}", uniswap_v2_pool.address).into(),
                );

                pool_map.insert(
                    String::from("token_a"),
                    format!("{:?}", uniswap_v2_pool.token_a).into(),
                );

                pool_map.insert(
                    String::from("token_a_decimals"),
                    uniswap_v2_pool.token_a_decimals.into(),
                );

                pool_map.insert(
                    String::from("token_b"),
                    format!("{:?}", uniswap_v2_pool.token_b).into(),
                );

                pool_map.insert(
                    String::from("token_b_decimals"),
                    uniswap_v2_pool.token_b_decimals.into(),
                );

                pool_map.insert(String::from("fee"), uniswap_v2_pool.fee.into());

                pools_array.push(pool_map.into());
            }

            Pool::UniswapV3(uniswap_v3_pool) => {
                pool_map.insert(
                    String::from("dex_variant"),
                    String::from("UniswapV3").into(),
                );

                pool_map.insert(
                    String::from("address"),
                    format!("{:?}", uniswap_v3_pool.address).into(),
                );

                pool_map.insert(
                    String::from("token_a"),
                    format!("{:?}", uniswap_v3_pool.token_a).into(),
                );

                pool_map.insert(
                    String::from("token_a_decimals"),
                    uniswap_v3_pool.token_a_decimals.into(),
                );

                pool_map.insert(
                    String::from("token_b"),
                    format!("{:?}", uniswap_v3_pool.token_b).into(),
                );

                pool_map.insert(
                    String::from("token_b_decimals"),
                    uniswap_v3_pool.token_b_decimals.into(),
                );

                pool_map.insert(String::from("fee"), uniswap_v3_pool.fee.into());

                pools_array.push(pool_map.into());
            }
        }
    }

    checkpoint.insert(String::from("pools"), pools_array.into());

    let checkpoint_file_name = String::from("./") + &checkpoint_file_name + ".json";

    std::fs::write(
        checkpoint_file_name,
        serde_json::to_string_pretty(&checkpoint).unwrap(),
    )
    .unwrap();
}
