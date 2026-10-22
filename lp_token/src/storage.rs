use soroban_sdk::{contracttype, Address, Env};

/// ~5s ledgers.
pub const DAY_IN_LEDGERS: u32 = 17_280;
pub const INSTANCE_BUMP_AMOUNT: u32 = 30 * DAY_IN_LEDGERS;
pub const INSTANCE_LIFETIME_THRESHOLD: u32 = INSTANCE_BUMP_AMOUNT - 7 * DAY_IN_LEDGERS;
pub const BALANCE_BUMP_AMOUNT: u32 = 90 * DAY_IN_LEDGERS;
pub const BALANCE_LIFETIME_THRESHOLD: u32 = BALANCE_BUMP_AMOUNT - 30 * DAY_IN_LEDGERS;

#[contracttype]
#[derive(Clone)]
pub struct AllowanceKey {
    pub from: Address,
    pub spender: Address,
}

#[contracttype]
#[derive(Clone)]
pub struct AllowanceValue {
    pub amount: i128,
    pub expiration_ledger: u32,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// The only address allowed to mint.
    Pool,
    /// Admin allowed to rotate the pool pin.
    Admin,
    Decimals,
    Name,
    Symbol,
    Balance(Address),
    /// Temporary storage, live until the allowance's expiration ledger.
    Allowance(AllowanceKey),
}

pub fn extend_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
}

pub fn read_balance(env: &Env, id: &Address) -> i128 {
    let key = DataKey::Balance(id.clone());
    match env.storage().persistent().get::<_, i128>(&key) {
        Some(balance) => {
            env.storage().persistent().extend_ttl(
                &key,
                BALANCE_LIFETIME_THRESHOLD,
                BALANCE_BUMP_AMOUNT,
            );
            balance
        }
        None => 0,
    }
}

pub fn write_balance(env: &Env, id: &Address, amount: i128) {
    let key = DataKey::Balance(id.clone());
    env.storage().persistent().set(&key, &amount);
    env.storage()
        .persistent()
        .extend_ttl(&key, BALANCE_LIFETIME_THRESHOLD, BALANCE_BUMP_AMOUNT);
}

pub fn read_allowance(env: &Env, from: &Address, spender: &Address) -> AllowanceValue {
    let key = DataKey::Allowance(AllowanceKey {
        from: from.clone(),
        spender: spender.clone(),
    });
    match env.storage().temporary().get::<_, AllowanceValue>(&key) {
        Some(allowance) if allowance.expiration_ledger >= env.ledger().sequence() => allowance,
        _ => AllowanceValue {
            amount: 0,
            expiration_ledger: 0,
        },
    }
}

pub fn write_allowance(
    env: &Env,
    from: &Address,
    spender: &Address,
    amount: i128,
    expiration_ledger: u32,
) {
    let key = DataKey::Allowance(AllowanceKey {
        from: from.clone(),
        spender: spender.clone(),
    });
    env.storage().temporary().set(&key, &AllowanceValue {
        amount,
        expiration_ledger,
    });
    // Temporary entries are only bumped while they are still live.
    if expiration_ledger > env.ledger().sequence() {
        env.storage()
            .temporary()
            .extend_ttl(&key, expiration_ledger - env.ledger().sequence(), expiration_ledger - env.ledger().sequence());
    }
}
