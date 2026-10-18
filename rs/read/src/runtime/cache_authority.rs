//! Fail-closed cache authority. Database metadata alone is not activation proof.
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authority {
    pub database_epoch: String,
    pub primary_identity: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FenceToken {
    authority: Authority,
    fence: u64,
}

#[derive(Default)]
struct State {
    authority: Option<Authority>,
    fence: u64,
}

#[derive(Default)]
pub struct CacheAuthority(Mutex<State>);

impl CacheAuthority {
    /// Called only after the canonical deploy/recovery owner proves old-primary
    /// fencing. No database-only discovery path can call this implicitly.
    pub fn activate(&self, authority: Authority) -> Result<(), &'static str> {
        if authority.database_epoch.is_empty()
            || authority.primary_identity.is_empty()
            || authority.revision == 0
        {
            return Err("unproved cache authority");
        }
        let mut state = self.0.lock().map_err(|_| "cache authority poisoned")?;
        state.fence = state.fence.checked_add(1).ok_or("cache fence exhausted")?;
        state.authority = Some(authority);
        Ok(())
    }

    pub fn revoke(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.authority = None;
            state.fence = state.fence.saturating_add(1);
        }
    }

    pub fn token(&self, database: &Authority) -> Option<FenceToken> {
        let state = self.0.lock().ok()?;
        (state.authority.as_ref() == Some(database)).then(|| FenceToken {
            authority: database.clone(),
            fence: state.fence,
        })
    }

    pub fn accepts(&self, token: &FenceToken) -> bool {
        self.token(&token.authority).as_ref() == Some(token)
    }
}
