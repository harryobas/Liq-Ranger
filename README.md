# Liq-Ranger ⚡️🤖  
*Cross-protocol liquidation bot for Aave & Morpho Blue on Polygon PoS*  

---

## 🔍 Overview  
**Liq-Ranger** is a cross-protocol liquidation bot designed to keep lending markets healthy while capturing profitable liquidation opportunities.  
It monitors borrower positions on **Aave v3** and **Morpho Blue**, focusing on **USDC, USDT, and DAI markets**, and executes flash-loan–powered liquidations when positions become unsafe.  

By leveraging efficient watchlist updates, on-chain event tracking, and automated profit reinvestment strategies, **Liq-Ranger** secures markets and maximizes yield for the operator.  

---

## ✨ Features  
- 🏦 **Cross-Protocol Support**: Works on both **Aave v3** and **Morpho Blue**  
- ⚡ **Flash Loan Powered**: Repay 100% of debt instantly during liquidation  
- 🔄 **Automated Watchlist Updates**: Maintains borrower lists from subgraphs & on-chain events  
- 📊 **Health Factor / LTV Checks**: Detects liquidatable positions in real time  
- 💰 **Profit Reinvestment**: Optionally loops profits back into yield strategies  
- 🎯 **Focus Mode**: Filters only **USDC, USDT, and DAI** reserves for Aave, and **USDC/USDT loan markets** for Morpho Blue  

---

## 🛠 Tech Stack  
- **Rust** – core logic, async runtime (`tokio`)  
- **ethers-rs** – Ethereum client, contract bindings  
- **Aave v3 Contracts** – lending & liquidation targets  
- **Morpho Blue** – isolated lending markets  
- **Uniswap Router** – swap execution for seized collateral  
- **Subgraph APIs** – for initial bootstrap of watchlists  

---

## 🚀 Getting Started  

### 1️⃣ Prerequisites  
- Install [Rust](https://www.rust-lang.org/) (latest stable)  
- Have access to a Polygon PoS RPC endpoint  
- Create an `.env` file with the following:  
  ```env
  PRIVATE_KEY=your_private_key
  RPC_URL=https://polygon-rpc.com
  SUBGRAPH_URL=https://api.thegraph.com/subgraphs/name/...
  SUBGRAPH_API_KEY=your_subgraph_key
  LENDING_POOL=0x...
  FLASH_LIQUIDATOR=0x...
  AAVE_ORACLE=0x...
  DEX_ROUTER=0x...
  UIPOOL_DATA=0x...
  POOL_ADDRESS_PROVIDER=0x...

### 2️⃣ Build

```
cargo build --release

```

