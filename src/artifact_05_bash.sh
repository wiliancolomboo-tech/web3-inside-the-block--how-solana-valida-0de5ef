# 1. Clone repository & install dependencies
git clone https://github.com/sponsor/solana-block-ordering-forensics.git
cd solana-block-ordering-forensics

# 2. Build and run Rust Scheduler Engine & Test Suite
cargo build --release
cargo test -- --nocapture

# 3. Launch TypeScript Monitoring Agent
npm install
npm run build
npm start