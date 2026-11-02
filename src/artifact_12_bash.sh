$ cargo test -- --nocapture
    Finished test [unoptimized + debuginfo] target(s) in 0.42s
     Running unittests src/lib.rs
     Running tests/scheduler_tests.rs

running 4 tests
test test_priority_fee_ordering_precedence ... ok
test test_account_compute_unit_limit_saturation ... ok
test test_block_compute_unit_limit_saturation ... ok
test test_jito_bundle_atomicity_and_top_of_block_slotting ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out