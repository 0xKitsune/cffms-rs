use std::{
    ops::{Div, Mul},
    sync::Arc,
};

use ethers::{
    providers::{JsonRpcClient, Provider},
    types::{H160, U256},
};

use crate::{abi, error::PairSyncError};

use super::{convert_to_common_decimals, convert_to_decimals};

#[derive(Clone, Copy)]
pub struct UniswapV2Pool {
    pub address: H160,
    pub token_a: H160,
    pub token_a_decimals: u8,
    pub token_b: H160,
    pub token_b_decimals: u8,
    pub a_to_b: bool,
    pub reserve_0: u128,
    pub reserve_1: u128,
    pub fee: u32,
}

impl UniswapV2Pool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        address: H160,
        token_a: H160,
        token_a_decimals: u8,
        token_b: H160,
        token_b_decimals: u8,
        a_to_b: bool,
        reserve_0: u128,
        reserve_1: u128,
        fee: u32,
    ) -> UniswapV2Pool {
        UniswapV2Pool {
            address,
            token_a,
            token_a_decimals,
            token_b,
            token_b_decimals,
            a_to_b,
            reserve_0,
            reserve_1,
            fee,
        }
    }

    //Creates a new instance of the pool from the pair address, and syncs the pool data
    pub async fn new_from_address<P: 'static + JsonRpcClient>(
        pair_address: H160,
        provider: Arc<Provider<P>>,
    ) -> Result<Self, PairSyncError<P>> {
        let mut pool = UniswapV2Pool {
            address: pair_address,
            token_a: H160::zero(),
            token_a_decimals: 0,
            token_b: H160::zero(),
            token_b_decimals: 0,
            a_to_b: false,
            reserve_0: 0,
            reserve_1: 0,
            fee: 300,
        };

        pool.token_a = pool.get_token_0(pair_address, provider.clone()).await?;
        pool.token_b = pool.get_token_1(pair_address, provider.clone()).await?;
        pool.a_to_b = true;

        (pool.token_a_decimals, pool.token_b_decimals) =
            pool.get_token_decimals(provider.clone()).await?;

        (pool.reserve_0, pool.reserve_1) = pool.get_reserves(provider).await?;

        Ok(pool)
    }

    pub async fn get_pool_data<P: 'static + JsonRpcClient>(
        &mut self,
        provider: Arc<Provider<P>>,
    ) -> Result<(), PairSyncError<P>> {
        self.token_a = self.get_token_0(self.address, provider.clone()).await?;
        self.token_b = self.get_token_1(self.address, provider.clone()).await?;
        self.a_to_b = true;

        (self.token_a_decimals, self.token_b_decimals) =
            self.get_token_decimals(provider.clone()).await?;

        Ok(())
    }

    pub async fn get_reserves<P: JsonRpcClient>(
        &self,
        provider: Arc<Provider<P>>,
    ) -> Result<(u128, u128), PairSyncError<P>> {
        //Initialize a new instance of the Pool
        let v2_pair = abi::IUniswapV2Pair::new(self.address, provider);

        // Make a call to get the reserves
        let (reserve_0, reserve_1, _) = match v2_pair.get_reserves().call().await {
            Ok(result) => result,

            Err(contract_error) => return Err(PairSyncError::ContractError(contract_error)),
        };

        Ok((reserve_0, reserve_1))
    }

    pub async fn sync_pool<P: 'static + JsonRpcClient>(
        &mut self,
        provider: Arc<Provider<P>>,
    ) -> Result<(), PairSyncError<P>> {
        (self.reserve_0, self.reserve_1) = self.get_reserves(provider).await?;

        Ok(())
    }

    pub async fn get_token_decimals<P: 'static + JsonRpcClient>(
        &mut self,
        provider: Arc<Provider<P>>,
    ) -> Result<(u8, u8), PairSyncError<P>> {
        let token_a_decimals = abi::IErc20::new(self.token_a, provider.clone())
            .decimals()
            .call()
            .await?;

        let token_b_decimals = abi::IErc20::new(self.token_b, provider)
            .decimals()
            .call()
            .await?;

        Ok((token_a_decimals, token_b_decimals))
    }

    pub async fn get_token_0<P: JsonRpcClient>(
        &self,
        pair_address: H160,
        provider: Arc<Provider<P>>,
    ) -> Result<H160, PairSyncError<P>> {
        let v2_pair = abi::IUniswapV2Pair::new(pair_address, provider);

        let token0 = match v2_pair.token_0().call().await {
            Ok(result) => result,
            Err(contract_error) => return Err(PairSyncError::ContractError(contract_error)),
        };

        Ok(token0)
    }

    pub async fn get_token_1<P: JsonRpcClient>(
        &self,
        pair_address: H160,
        provider: Arc<Provider<P>>,
    ) -> Result<H160, PairSyncError<P>> {
        let v2_pair = abi::IUniswapV2Pair::new(pair_address, provider);

        let token1 = match v2_pair.token_1().call().await {
            Ok(result) => result,
            Err(contract_error) => return Err(PairSyncError::ContractError(contract_error)),
        };

        Ok(token1)
    }

    pub fn calculate_price(&self, base_token: H160) -> f64 {
        if self.a_to_b {
            let reserve_0 = self.reserve_0 as f64 / 10f64.powf(self.token_a_decimals.into());
            let reserve_1 = self.reserve_1 as f64 / 10f64.powf(self.token_b_decimals.into());

            if base_token == self.token_a {
                reserve_0 / reserve_1
            } else {
                reserve_1 / reserve_0
            }
        } else {
            //else if b to a
            let reserve_0 = self.reserve_0 as f64 / 10f64.powf(self.token_b_decimals.into());
            let reserve_1 = self.reserve_1 as f64 / 10f64.powf(self.token_a_decimals.into());

            if base_token == self.token_a {
                reserve_1 / reserve_0
            } else {
                reserve_0 / reserve_1
            }
        }
    }

    pub fn address(&self) -> H160 {
        self.address
    }

    pub async fn simulate_swap(&self, token_in: H160, amount_in: u128) -> U256 {
        let (reserve_0, reserve_1, common_decimals) = convert_to_common_decimals(
            self.reserve_0,
            self.token_a_decimals,
            self.reserve_1,
            self.token_b_decimals,
        );

        //Apply fee on amount in
        //Fee will always be .3% for Univ2
        let amount_in = amount_in.mul(997).div(1000);

        // x * y = k
        // (x + ∆x) * (y - ∆y) = k
        // y - (k/(x + ∆x)) = ∆y
        let k = reserve_0 * reserve_1;

        if self.token_a == token_in {
            if self.a_to_b {
                U256::from(convert_to_decimals(
                    reserve_1 - (k * (self.reserve_0 + amount_in)),
                    common_decimals,
                    self.token_b_decimals,
                ))
            } else {
                U256::from(convert_to_decimals(
                    reserve_0 - (k * (self.reserve_1 + amount_in)),
                    common_decimals,
                    self.token_a_decimals,
                ))
            }
        } else if self.a_to_b {
            U256::from(convert_to_decimals(
                reserve_0 - (k * (self.reserve_1 + amount_in)),
                common_decimals,
                self.token_a_decimals,
            ))
        } else {
            U256::from(convert_to_decimals(
                reserve_1 - (k * (self.reserve_0 + amount_in)),
                common_decimals,
                self.token_b_decimals,
            ))
        }
    }
}
