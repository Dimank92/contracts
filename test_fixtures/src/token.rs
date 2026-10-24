//! Token test fixture: deploys a Stellar Asset Contract and exposes
//! mint/balance/approve helpers so test suites don't reimplement them.
//!
//! # Example
//! ```rust,ignore
//! let env = Env::default();
//! env.mock_all_auths();
//! let token = TokenFixture::deploy(&env);
//! let user = Address::generate(&env);
//! token.mint(&env, &user, 100_000_000);
//! assert_eq!(token.balance(&env, &user), 100_000_000);
//! ```

use soroban_sdk::{token, Address, Env};

/// A deployed Stellar Asset Contract with a minting authority.
pub struct TokenFixture {
    /// The SAC contract address.
    pub address: Address,
    /// The admin address (holds mint authority).
    pub admin: Address,
}

impl TokenFixture {
    /// Deploy a fresh SAC token with a generated admin in `env`.
    pub fn deploy(env: &Env) -> Self {
        let admin = Address::generate(env);
        let contract = env.register_stellar_asset_contract_v2(admin.clone());
        TokenFixture {
            address: contract.address(),
            admin,
        }
    }

    /// Mint `amount` tokens to `to`. Requires `env.mock_all_auths()` or admin auth.
    pub fn mint(&self, env: &Env, to: &Address, amount: i128) {
        token::StellarAssetClient::new(env, &self.address).mint(to, &amount);
    }

    /// Query the token balance of `of`.
    pub fn balance(&self, env: &Env, of: &Address) -> i128 {
        token::Client::new(env, &self.address).balance(of)
    }

    /// Approve `spender` to spend up to `amount` on behalf of `from`.
    /// The allowance expires after 100 ledgers from the current sequence.
    pub fn approve(&self, env: &Env, from: &Address, spender: &Address, amount: i128) {
        let expiry = env.ledger().sequence() + 100;
        token::Client::new(env, &self.address).approve(from, spender, &amount, &expiry);
    }
}
