use ethers::{
    providers::Middleware,
    types::{Address, Block, Bytes, TxHash, U256},
};
use revm::{
    db::{CacheDB, Database, DatabaseRef, EthersDB},
    primitives::{
        AccountInfo, Address as rAddress, BlockEnv, Bytecode, Bytes as rBytes, TransactTo, TxEnv,
        B256 as rB256, U256 as rU256,
    },
};
use std::sync::{Arc, Mutex};

/// Thread-safe wrapper over `EthersDB<M>` to satisfy `DatabaseRef` cleanly.
pub struct SharedEthersDB<M: Middleware>(pub Arc<Mutex<EthersDB<M>>>);

// Manual Clone implementation without requiring `M: Clone`
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
    type Error = <EthersDB<M> as Database>::Error;

    fn basic(&self, address: rAddress) -> Result<Option<AccountInfo>, Self::Error> {
        self.0.lock().unwrap().basic(address)
    }

    fn code_by_hash(&self, code_hash: rB256) -> Result<Bytecode, Self::Error> {
        self.0.lock().unwrap().code_by_hash(code_hash)
    }

    fn storage(&self, address: rAddress, index: rU256) -> Result<rU256, Self::Error> {
        self.0.lock().unwrap().storage(address, index)
    }

    fn block_hash(&self, number: rU256) -> Result<rB256, Self::Error> {
        self.0.lock().unwrap().block_hash(number)
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
    BlockEnv {
        number: rU256::from(block.number.unwrap_or_default().as_u64()),
        timestamp: rU256::from(block.timestamp.as_u64()),
        coinbase: block.author.unwrap_or_default().0.into(),
        gas_limit: rU256::from(block.gas_limit.as_u64()),
        basefee: rU256::from(block.base_fee_per_gas.unwrap_or_default().as_u64()),
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
    let gas_price = block
        .base_fee_per_gas
        .unwrap_or_else(|| U256::from(30_000_000_000u64));

    TxEnv {
        caller: from.0.into(),
        transact_to: TransactTo::Call(to.0.into()),
        data: rBytes::from(calldata.to_vec()),
        value: rU256::from_limbs(value.0),
        gas_limit: block.gas_limit.as_u64(),
        gas_price: rU256::from_limbs(gas_price.0),
        chain_id: Some(chain_id),
        ..Default::default()
    }
}
