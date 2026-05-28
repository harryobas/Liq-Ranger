# crap4rs v0.6.0 — CRAP Score Analysis

## Summary

**Result:** FAIL · **Functions:** 234 · **Above threshold (15):** 36

| Metric     | Worst | Average | Median |
|------------|------:|--------:|-------:|
| CRAP       | 1482.00 |   28.14 |   2.00 |
| Complexity |    38 |     2.9 |    1.0 |
| Coverage   |  0.0% |   52.7% | 100.0% |

**Risk distribution:** low 184 · acceptable 14 · moderate 8 · high 28

## Failures (top 10 of 36 above threshold 15)

| File | Function | CC | Cov% | CRAP | Risk |
|------|----------|----|------|------|------|
| aave/aave_liquidator.rs | AaveLiquidator::analyze_portfolio | 38 | 0.0 | 1482.00 | high |
| bootstrap_engine/aave_bootstrap.rs | AaveBootstrap::run | 29 | 0.0 | 870.00 | high |
| bootstrap_engine/morpho_bootstrap.rs | MorphoBootstrap::run | 29 | 0.0 | 870.00 | high |
| morpho/morpho_liquidator.rs | MorphoLiquidator::analyze_borrower | 17 | 0.0 | 306.00 | high |
| lib.rs | start_liquidation_engines | 16 | 0.0 | 272.00 | high |
| profit_distributor.rs | ProfitDistributor::distribute_all_assets | 14 | 0.0 | 210.00 | high |
| aave/helpers.rs | select_collateral_candidates | 13 | 0.0 | 182.00 | high |
| compound/helpers.rs | base_amount_for_collateral | 13 | 0.0 | 182.00 | high |
| common/simulation_sandbox.rs | AnvilSandbox::simulate_tx | 12 | 0.0 | 156.00 | high |
| aave/aave_liquidator.rs | AaveLiquidator::run | 11 | 0.0 | 132.00 | high |
