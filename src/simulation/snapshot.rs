use ethers::{
    providers::{Middleware, ProviderError},
    types::{Address, Block, Bytes, TxHash, U256},
};
use revm::{
    db::{CacheDB, Database, DatabaseRef, EthersDB},
    primitives::{
        AccountInfo, Address as rAddress, BlockEnv, Bytecode, Bytes as rBytes, TransactTo, TxEnv,
        B256 as rB256, U256 as rU256,
    },
};
use std::sync::{Arc, RwLock};

/// Thread-safe wrapper over `EthersDB<M>` using `RwLock` to allow lock-free cache access
/// and minimize block stalls during RPC lookups.
pub struct SharedEthersDB<M: Middleware>(pub Arc<RwLock<EthersDB<M>>>);

impl<M: Middleware> Clone for SharedEthersDB<M> {
    fn clone(&self) -> Self {
        SharedEthersDB(Arc::clone(&self.0))
    }
}

impl<M> DatabaseRef for SharedEthersDB<M>
where
    M: Middleware + Send + Sync + 'static,
    <M as Middleware>::Error: 'static,
{
    //type Error = <EthersDB<M> as Database>::Error;
    type Error = ProviderError;

    fn basic(&self, address: rAddress) -> Result<Option<AccountInfo>, Self::Error> {
        let mut db = self.0.write().map_err(|e| {
            ProviderError::CustomError(e.to_string())
        })?;

        db.basic(address).map_err(|_| {
            ProviderError::CustomError("EthersDB query failed".to_string())
        })
    }

    fn code_by_hash(&self, code_hash: rB256) -> Result<Bytecode, Self::Error> {
        let mut db = self.0.write().map_err(|e| {
            ProviderError::CustomError(e.to_string())
        })?;

        db.code_by_hash(code_hash).map_err(|_| {
            ProviderError::CustomError("EthersDB query failed".to_string())
        })
    }

    fn storage(&self, address: rAddress, index: rU256) -> Result<rU256, Self::Error> {
        let mut db = self.0.write().map_err(|e| {
            ProviderError::CustomError(e.to_string())
        })?;

        db.storage(address, index).map_err(|_| {
            ProviderError::CustomError("EthersDB query failed".to_string())
        })
    }

    fn block_hash(&self, number: rU256) -> Result<rB256, Self::Error> {
        let mut db = self.0.write().map_err(|e| {
            ProviderError::CustomError(e.to_string())
        })?;

        db.block_hash(number).map_err(|_| {
            ProviderError::CustomError("EthersDB query failed".to_string())
        })
    }
}

pub struct BlockSnapshot<M>
where
    M: Middleware + Send + Sync + 'static,
    <M as Middleware>::Error: 'static,
{
    pub block_number: u64,
    pub block: Arc<Block<TxHash>>,
    pub db: Arc<CacheDB<SharedEthersDB<M>>>,
}

pub fn build_block_env(block: &Block<TxHash>) -> BlockEnv {
    let basefee = block.base_fee_per_gas.unwrap_or_default();
    let prevrandao = block.mix_hash.unwrap_or_default();

    BlockEnv {
        number: rU256::from(block.number.unwrap_or_default().as_u64()),
        timestamp: rU256::from(block.timestamp.as_u64()),
        coinbase: block.author.unwrap_or_default().0.into(),
        gas_limit: rU256::from(block.gas_limit.as_u64()),
        basefee: rU256::from_limbs(basefee.0),
        prevrandao: Some(rB256::from(prevrandao.0)),
        ..Default::default()
    }
}

pub fn build_tx_env(
    from: Address,
    to: Address,
    calldata: Bytes,
    value: U256,
    block: &Block<TxHash>,
    chain_id: u64,
) -> TxEnv {
    let base_fee = block
        .base_fee_per_gas
        .unwrap_or_else(|| U256::from(30_000_000_000u64));

    // Priority fee default (1.5 Gwei for flashbot execution context)
    let priority_fee = U256::from(1_500_000_000u64);
    let max_fee_per_gas = base_fee + priority_fee;

    TxEnv {
        caller: from.0.into(),
        transact_to: TransactTo::Call(to.0.into()),
        data: rBytes::from(calldata.to_vec()),
        value: rU256::from_limbs(value.0),
        gas_limit: block.gas_limit.as_u64(),
        gas_price: rU256::from_limbs(max_fee_per_gas.0),
        gas_priority_fee: Some(rU256::from_limbs(priority_fee.0)),
        chain_id: Some(chain_id),
        ..Default::default()
    }
}