CREATE TABLE IF NOT EXISTS liquidations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tx_hash TEXT NOT NULL UNIQUE,
    protocol TEXT NOT NULL,         -- e.g., 'AaveV3', 'Morpho'
    borrower TEXT NOT NULL,
    profit_asset TEXT NOT NULL,
    profit_symbol TEXT,  
    collateral_asset TEXT NOT NULL,
    collateral_symbol TEXT, 
    profit_amount REAL NOT NULL,    
    block_number INTEGER NOT NULL,
    timestamp INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS distributions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tx_hash TEXT NOT NULL UNIQUE,
    asset TEXT NOT NULL,
    asset_symbol TEXT,
    amount REAL NOT NULL,
    owner_share REAL NOT NULL,
    breet_share REAL NOT NULL,
    timestamp INTEGER NOT NULL
);

-- Indexing for faster Grafana queries and Distributor discovery
CREATE INDEX IF NOT EXISTS idx_liq_profit_asset ON liquidations(profit_asset);
CREATE INDEX IF NOT EXISTS idx_liq_collateral_asset ON liquidations(collateral_asset);-- Add up migration script here
CREATE INDEX IF NOT EXISTS idx_liquidations_timestamp ON liquidations(timestamp);
CREATE INDEX IF NOT EXISTS idx_distributions_timestamp ON distributions(timestamp);