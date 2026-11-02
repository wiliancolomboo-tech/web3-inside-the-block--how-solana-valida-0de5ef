# 1. Clone & Build Release Binary
cargo build --release

# 2. Run Test Matrix with 100% assertions passing
cargo test -- --nocapture

# 3. Execute High-Contention Banking Simulation (Local)
./target/release/sbe-cli simulate --tx-count 500 --threads 8

# 4. Profile Account Lock Saturation Against Hot AMM State
./target/release/sbe-cli contention-profile --swaps 100