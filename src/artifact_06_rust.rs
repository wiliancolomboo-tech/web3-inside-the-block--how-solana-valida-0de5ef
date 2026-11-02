use crate::types::Pubkey;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default)]
pub struct LockTable {
    write_locks: HashSet<Pubkey>,
    read_locks: HashMap<Pubkey, u32>,
    account_compute_accumulated: HashMap<Pubkey, u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LockError {
    WriteConflict(Pubkey),
    ReadConflict(Pubkey),
    AccountComputeExceeded(Pubkey),
}

impl LockTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Try to acquire read/write locks and assert CU limits per account
    pub fn try_lock(
        &mut self,
        read_keys: &[Pubkey],
        write_keys: &[Pubkey],
        cu_required: u64,
        max_cu_per_account: u64,
    ) -> Result<(), LockError> {
        // 1. Check write lock collisions against existing write locks
        for key in write_keys {
            if self.write_locks.contains(key) {
                return Err(LockError::WriteConflict(*key));
            }
            if self.read_locks.get(key).copied().unwrap_or(0) > 0 {
                return Err(LockError::ReadConflict(*key));
            }
            let current_cu = self.account_compute_accumulated.get(key).copied().unwrap_or(0);
            if current_cu.saturating_add(cu_required) > max_cu_per_account {
                return Err(LockError::AccountComputeExceeded(*key));
            }
        }

        // 2. Check read lock collisions against existing write locks
        for key in read_keys {
            if self.write_locks.contains(key) {
                return Err(LockError::WriteConflict(*key));
            }
        }

        // 3. Commit locks
        for key in write_keys {
            self.write_locks.insert(*key);
            let entry = self.account_compute_accumulated.entry(*key).or_insert(0);
            *entry = entry.saturating_add(cu_required);
        }

        for key in read_keys {
            let count = self.read_locks.entry(*key).or_insert(0);
            *count = count.saturating_add(1);
        }

        Ok(())
    }

    /// Release locks when transaction execution completes
    pub fn unlock(&mut self, read_keys: &[Pubkey], write_keys: &[Pubkey]) {
        for key in write_keys {
            self.write_locks.remove(key);
        }
        for key in read_keys {
            if let Some(count) = self.read_locks.get_mut(key) {
                if *count <= 1 {
                    self.read_locks.remove(key);
                } else {
                    *count -= 1;
                }
            }
        }
    }

    pub fn get_account_cu(&self, pubkey: &Pubkey) -> u64 {
        self.account_compute_accumulated.get(pubkey).copied().unwrap_or(0)
    }

    pub fn reset_slot(&mut self) {
        self.write_locks.clear();
        self.read_locks.clear();
        self.account_compute_accumulated.clear();
    }
}