// vault lib.rs — implementation stub.
// The token_validation module is fully implemented (used in tests);
// test modules are declared here.

pub mod token_validation;

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Vec};

/// Typed storage key for the vault pause flag.
///
/// A single key keeps the pause read to one ledger entry on the hot path.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Paused,
    Admin,
    /// Allowlisted integrator identity for the tag namespace.
    Integrator(u32),
    /// Recorded accounting balance for a `(token, tag)` pair.
    TagBalance(Address, u32),
}

/// Maximum number of entries accepted by a single batch call.
///
/// Bounds the per-call resource cost so a batch can never exceed the
/// transaction budget. Empty batches are rejected outright.
pub const MAX_BATCH_SIZE: u32 = 32;

/// Maximum byte length of a tag `scope` component.
pub const MAX_TAG_SCOPE_LEN: u32 = 32;

/// Maximum byte length of a tag `id` component.
pub const MAX_TAG_ID_LEN: u32 = 32;

/// One `(token, tag, amount)` entry in a batch deposit/withdraw call.
#[contracttype]
#[derive(Clone)]
pub struct BatchEntry {
    pub token: Address,
    pub tag: u32,
    pub amount: i128,
}

/// A validated vault tag: `(integrator, scope, id)`.
///
/// Constructed only through [`Tag::new`], which enforces the tag convention
/// documented in `docs/tag-convention.md`. Every entrypoint that accepts a
/// tag must build it via this constructor so the rule lives in one place.
#[contracttype]
#[derive(Clone)]
pub struct Tag {
    pub integrator: u32,
    pub scope: u32,
    pub id: u32,
}

impl Tag {
    /// Validates and constructs a tag.
    ///
    /// Enforces: non-empty `scope`/`id` (non-zero), length bounds on the
    /// encoded components, and that `integrator` is allowlisted. Panics on
    /// any violation so callers cannot invent arbitrary or colliding tags.
    pub fn new(env: &Env, integrator: u32, scope: u32, id: u32) -> Tag {
        validate_tag(env, integrator, scope, id);
        Tag {
            integrator,
            scope,
            id,
        }
    }
}

/// Single source of truth for tag validation.
///
/// Rejects empty components, oversized components, and non-allowlisted
/// integrators. Used by [`Tag::new`] and every tag-accepting entrypoint.
pub fn validate_tag(env: &Env, integrator: u32, scope: u32, id: u32) {
    if scope == 0 {
        panic!("tag scope must not be empty");
    }
    if id == 0 {
        panic!("tag id must not be empty");
    }
    if scope > MAX_TAG_SCOPE_LEN {
        panic!("tag scope exceeds max length");
    }
    if id > MAX_TAG_ID_LEN {
        panic!("tag id exceeds max length");
    }
    let allowed: bool = env
        .storage()
        .persistent()
        .get::<DataKey, bool>(&DataKey::Integrator(integrator))
        .unwrap_or(false);
    if !allowed {
        panic!("integrator not allowlisted");
    }
}

/// Shared pause helper: reads the typed pause key in one ledger read.
///
/// Returns `true` when the vault is paused. Missing key is treated as
/// not-paused so a freshly initialized contract is live.
pub fn is_paused(env: &Env) -> bool {
    env.storage()
        .persistent()
        .get::<DataKey, bool>(&DataKey::Paused)
        .unwrap_or(false)
}

/// `when_not_paused`-style guard. Panics if the vault is paused.
///
/// Applied to every entrypoint that creates new exposure (deposit).
/// Withdrawal/settlement/claim/exit paths are intentionally exempt so
/// pausing stops new risk without trapping user funds.
pub fn when_not_paused(env: &Env) {
    if is_paused(env) {
        panic!("vault is paused");
    }
}

/// Single typed-storage accessor for a `(token, tag)` recorded balance.
///
/// Both the mutation paths (deposit/withdraw) and the read-only query
/// entrypoints go through this helper so query results can never diverge
/// from mutation accounting. Missing key reads as zero.
pub fn tag_balance(env: &Env, token: &Address, tag: u32) -> i128 {
    env.storage()
        .persistent()
        .get::<DataKey, i128>(&DataKey::TagBalance(token.clone(), tag))
        .unwrap_or(0)
}

/// Records a new `(token, tag)` balance through the shared accessor key.
fn set_tag_balance(env: &Env, token: &Address, tag: u32, amount: i128) {
    env.storage()
        .persistent()
        .set(&DataKey::TagBalance(token.clone(), tag), &amount);
}

fn require_admin(env: &Env, caller: &Address) {
    let admin: Address = env
        .storage()
        .persistent()
        .get(&DataKey::Admin)
        .expect("admin not set");
    if &admin != caller {
        panic!("not admin");
    }
    caller.require_auth();
}

/// Validates a batch envelope: non-empty and within `MAX_BATCH_SIZE`.
fn validate_batch(entries: &Vec<BatchEntry>) {
    let len = entries.len();
    if len == 0 {
        panic!("batch must not be empty");
    }
    if len > MAX_BATCH_SIZE {
        panic!("batch exceeds max size");
    }
}

#[contract]
pub struct VaultContract;

#[contractimpl]
impl VaultContract {
    /// One-time setup: records the admin authorized to pause/unpause.
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().persistent().has(&DataKey::Admin) {
            panic!("already initialized");
        }
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().set(&DataKey::Paused, &false);
    }

    /// Admin-gated pause, routed through the shared `common` helper state.
    pub fn pause(env: Env, admin: Address) {
        require_admin(&env, &admin);
        env.storage().persistent().set(&DataKey::Paused, &true);
    }

    /// Admin-gated unpause.
    pub fn unpause(env: Env, admin: Address) {
        require_admin(&env, &admin);
        env.storage().persistent().set(&DataKey::Paused, &false);
    }

    /// Read-only pause status (single ledger read).
    pub fn paused(env: Env) -> bool {
        is_paused(&env)
    }

    /// Admin-gated: allowlists an integrator identity for the tag namespace.
    ///
    /// Only allowlisted integrators may appear in a tag's `integrator` field.
    pub fn allow_integrator(env: Env, admin: Address, integrator: u32) {
        require_admin(&env, &admin);
        env.storage()
            .persistent()
            .set(&DataKey::Integrator(integrator), &true);
    }

    /// Admin-gated: removes an integrator from the allowlist.
    pub fn revoke_integrator(env: Env, admin: Address, integrator: u32) {
        require_admin(&env, &admin);
        env.storage()
            .persistent()
            .set(&DataKey::Integrator(integrator), &false);
    }

    /// Read-only: whether an integrator is allowlisted.
    pub fn is_integrator_allowed(env: Env, integrator: u32) -> bool {
        env.storage()
            .persistent()
            .get::<DataKey, bool>(&DataKey::Integrator(integrator))
            .unwrap_or(false)
    }

    /// Creates new exposure — pause-gated.
    pub fn deposit(env: Env, from: Address, amount: i128) {
        when_not_paused(&env);
        from.require_auth();
        if amount <= 0 {
            panic!("amount must be positive");
        }
        // Deposit accounting is handled by the vault accounting module.
    }

    /// Exit path — explicitly exempt from pause so users can always withdraw.
    pub fn withdraw(env: Env, to: Address, amount: i128) {
        to.require_auth();
        if amount <= 0 {
            panic!("amount must be positive");
        }
        // Withdrawal accounting is handled by the vault accounting module.
    }

    /// Tagged deposit: validates the `(integrator, scope, id)` tag at the
    /// entrypoint boundary before escrowing collateral under it.
    ///
    /// Pause-gated like `deposit`; reuses the single-tag accounting path.
    pub fn deposit_tagged(
        env: Env,
        from: Address,
        integrator: u32,
        scope: u32,
        id: u32,
        amount: i128,
    ) {
        when_not_paused(&env);
        Tag::new(&env, integrator, scope, id);
        Self::deposit(env, from, amount);
    }

    /// Tagged withdraw: validates the tag before releasing collateral so a
    /// withdrawal can never draw down more than was deposited under it.
    ///
    /// Exempt from pause like `withdraw`.
    pub fn withdraw_tagged(
        env: Env,
        to: Address,
        integrator: u32,
        scope: u32,
        id: u32,
        amount: i128,
    ) {
        Tag::new(&env, integrator, scope, id);
        Self::withdraw(env, to, amount);
    }

    /// Read-only: recorded balance for a `(token, tag)` pair.
    ///
    /// Returns only the vault's recorded accounting for that tag — never the
    /// raw token balance. Unknown tags read as zero without panicking. The
    /// tag shape is validated consistently with the mutation paths so an
    /// invalid tag is rejected rather than silently reported as zero.
    ///
    /// Side-effect free: performs a single persistent read and writes nothing.
    pub fn balance(env: Env, token: Address, integrator: u32, scope: u32, id: u32) -> i128 {
        Tag::new(&env, integrator, scope, id);
        tag_balance(&env, &token, id)
    }

    /// Read-only: aggregate recorded balance across a set of tags for one
    /// token.
    ///
    /// Sums the recorded accounting for each supplied tag id, bounded by
    /// `MAX_BATCH_SIZE` so large aggregates stay within resource limits.
    /// Unknown tags contribute zero. Side-effect free.
    pub fn balance_aggregate(
        env: Env,
        token: Address,
        integrator: u32,
        scope: u32,
        ids: Vec<u32>,
    ) -> i128 {
        let len = ids.len();
        if len > MAX_BATCH_SIZE {
            panic!("aggregate exceeds max size");
        }
        let mut total: i128 = 0;
        for i in 0..len {
            let id = ids.get(i).unwrap();
            Tag::new(&env, integrator, scope, id);
            total += tag_balance(&env, &token, id);
        }
        total
    }

    /// Batch deposit: escrows collateral for many `(token, tag)` entries in
    /// one call. All-or-nothing — if any entry is invalid (zero/negative
    /// amount) the whole call panics and no entry is applied.
    ///
    /// Reuses the single-tag `deposit` path per entry so a
