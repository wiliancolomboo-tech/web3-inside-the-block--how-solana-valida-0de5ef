# 1. Clone repository and verify environment
git clone https://github.com/enterprise-solana/solana-block-orderer.git
cd solana-block-orderer

# 2. Run Comprehensive Test Matrix
cargo test --verbose -- --nocapture

# 3. Build release binaries with optimized SIMD/AVX2 flags
RUSTFLAGS="-C target-cpu=native" cargo build --release

# 4. Run CLI in simulation inspection mode
./target/release/inspector simulate-slot \
  --slot 285910240 \
  --leader 9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM \
  --tx-count 2000 \
  --bundle-count 25