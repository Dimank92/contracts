#![no_std]

//! Zenith Streams — linear, per-second token streams with an optional
//! cliff, funded upfront by the funder (typically the treasury).
//!
//! `streamed(t) = total × (t − start) / (end − start)`, floored, clamped to
//! `[0, total]` and zero before `cliff`. Because every intermediate value
//! is floored and the recipient can only ever withdraw
//! `streamed(now) − withdrawn`, the total paid out can never exceed the
//! deposit (see the property tests in test.rs).
//!
//! The recipient withdraws at any time and may transfer the stream to a
//! new recipient. The funder or governance can cancel: the recipient is
//! paid everything accrued but not yet withdrawn, and the funder is
//! refunded the unstreamed remainder. Governance can also claw back a
//! stream (e.g. compromised or migrated grants) via a separately
//! authorized path that settles identically but emits a distinct event.

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env};

#[cfg(test)]
mod test;

mod error;
mod events;
pub mod math;
mod migrations;
mod types;

use error::Error;
pub use migrations::CURRENT_SCHEMA_VERSION;
use types::{DataKey, Stream};

#[contract]
pub struct Streams;

fn load(env: &Env, stream_id: u64) -> Stream {
    env.storage()
        .persistent()
        .get(&DataKey::Stream(stream_id))
        .unwrap_or_else(|| panic_with_error!(env, Error::StreamNotFound))
}

fn save(env: &Env, stream_id: u64, stream: &Stream) {
    env.storage()
        .persistent()
        .set(&DataKey::Stream(stream_id), stream);
}

/// Amount streamed to the recipient as of `now`, in `[0, total]`.
fn streamed_at(stream: &Stream, now: u64) -> i128 {
    if stream.canceled || now >= stream.end {
        return stream.total;
    }
    if now < stream.cliff || now <= stream.start {
        return 0;
    }
    math::mul_div_floor(
        stream.total,
        (now - stream.start) as i128,
        (stream.end - stream.start) as i128,
    )
}

/// Settles a stream: pays the recipient everything accrued but not yet
/// withdrawn and refunds the unstreamed remainder to the funder. The
/// stream is marked terminal in storage before any token transfer so a
/// re-entrant call cannot double-settle. Reuses `streamed_at` so the
/// split can never disagree with `withdraw` by a stroop.
fn settle(env: &Env, stream_id: u64, stream: &mut Stream) -> (i128, i128) {
    let streamed = streamed_at(stream, env.ledger().timestamp());
    let to_recipient = streamed - stream.withdrawn;
    let to_funder = stream.total - streamed;

    stream.total = streamed;
    stream.withdrawn = streamed;
    stream.canceled = true;
    save(env, stream_id, stream);

    let token = token::Client::new(env, &stream.token);
    let this = env.current_contract_address();
    if to_recipient > 0 {
        token.transfer(&this, &stream.recipient, &to_recipient);
    }
    if to_funder > 0 {
        token.transfer(&this, &stream.funder, &to_funder);
    }
    (to_recipient, to_funder)
}

#[contractimpl]
impl Streams {
    pub fn initialize(env: Env, governance: Address) {
        if env.storage().instance().has(&DataKey::Governance) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        env.storage()
            .instance()
            .set(&DataKey::Governance, &governance);
        env.storage().instance().set(&DataKey::StreamCounter, &0u64);
        zenith_common::migrations::init_schema(&env, CURRENT_SCHEMA_VERSION);
    }

    pub fn get_governance(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Governance).unwrap()
    }

    /// Creates a stream and pulls `total` of `token` from `funder` upfront.
    /// `start` may be in the past (the elapsed portion is withdrawable
    /// immediately, subject to the cliff). Zero-duration streams
    /// (`start == end`) are rejected. `revocable` is immutable after
    /// creation: an irrevocable stream can never be cancelled or clawed
    /// back.
    #[allow(clippy::too_many_arguments)]
    pub fn create_stream(
        env: Env,
        funder: Address,
        recipient: Address,
        token: Address,
        total: i128,
        start: u64,
        cliff: u64,
        end: u64,
        revocable: bool,
    ) -> u64 {
        funder.require_auth();
        migrations::require_current(&env);
        if total <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        if start >= end || cliff < start || cliff > end {
            panic_with_error!(&env, Error::InvalidSchedule);
        }

        token::Client::new(&env, &token).transfer(&funder, &env.current_contract_address(), &total);

        let stream_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::StreamCounter)
            .unwrap();
        env.storage()
            .instance()
            .set(&DataKey::StreamCounter, &(stream_id + 1));

        save(
            &env,
            stream_id,
            &Stream {
                funder: funder.clone(),
                recipient: recipient.clone(),
                token,
                total,
                start,
                cliff,
                end,
                withdrawn: 0,
                canceled: false,
                revocable,
            },
        );
        events::created(&env, stream_id, funder, recipient, total);
        stream_id
    }

    /// Recipient withdraws up to their currently accrued balance.
    pub fn withdraw(env: Env, stream_id: u64, amount: i128) {
        let mut stream = load(&env, stream_id);
        stream.recipient.require_auth();
        migrations::require_current(&env);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        let available = streamed_at(&stream, env.ledger().timestamp()) - stream.withdrawn;
        if amount > available {
            panic_with_error!(&env, Error::InsufficientBalance);
        }
        stream.withdrawn += amount;
        save(&env, stream_id, &stream);

        token::Client::new(&env, &stream.token).transfer(
            &env.current_contract_address(),
            &stream.recipient,
            &amount,
        );
        events::withdrawn(&env, stream_id, stream.recipient, amount);
    }

    /// Sender (funder) cancels a revocable stream. The recipient receives
    /// everything accrued but not yet withdrawn (zero if still before the
    /// cliff) and the funder is refunded the unstreamed remainder.
    pub fn cancel(env: Env, caller: Address, stream_id: u64) {
        caller.require_auth();
        migrations::require_current(&env);
        let mut stream = load(&env, stream_id);
        if caller != stream.funder {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if !stream.revocable {
            panic_with_error!(&env, Error::Irrevocable);
        }
        if stream.canceled {
            panic_with_error!(&env, Error::StreamCanceled);
        }

        let (to_recipient, to_funder) = settle(&env, stream_id, &mut stream);
        events::canceled(&env, stream_id, to_recipient, to_funder);
    }

    /// Governance clawback for compromised or migrated streams. Behaves
    /// identically to `cancel` but is separately authorized (governance
    /// only) and emits a distinct event for indexers.
    pub fn clawback(env: Env, stream_id: u64) {
        migrations::require_current(&env);
        Self::get_governance(env.clone()).require_auth();
        let mut stream = load(&env, stream_id);
        if !stream.revocable {
            panic_with_error!(&env, Error::Irrevocable);
        }
        if stream.canceled {
            panic_with_error!(&env, Error::StreamCanceled);
        }

        let (to_recipient, to_funder) = settle(&env, stream_id, &mut stream);
        events::clawed_back(&env, stream_id, to_recipient, to_funder);
    }

    /// Recipient hands the stream (including any accrued, unwithdrawn
    /// balance) to a new recipient.
    pub fn transfer(env: Env, stream_id: u64, new_recipient: Address) {
        let mut stream = load(&env, stream_id);
        stream.recipient.require_auth();
        migrations::require_current(&env);
        if stream.canceled {
            panic_with_error!(&env, Error::StreamCanceled);
        }
        let old = stream.recipient.clone();
        stream.recipient = new_recipient.clone();
        save(&env, stream_id, &stream);
        events::transferred(&env, stream_id, old, new_recipient);
    }

    // ── Schema migrations (docs/migrations.md) ──────────────────────────────

    /// Runs the registered steps `from+1 ..= to`. Governance only. Returns
    /// the version reached (less than `to` if a step paginated).
    pub fn migrate(env: Env, from: u32, to: u32) -> u32 {
        Self::get_governance(env.clone()).require_auth();
        zenith_common::migrations::migrate(
            &env,
            from,
            to,
            CURRENT_SCHEMA_VERSION,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    /// Continues a paginated migration from `cursor`. Permissionless, like
    /// `bump`: the target was fixed by `migrate` and the steps are code in
    /// this wasm, so a keeper can only advance it.
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> u32 {
        zenith_common::migrations::migrate_batch(
            &env,
            cursor,
            limit,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    pub fn schema_version(env: Env) -> u32 {
        zenith_common::migrations::schema_version(&env)
    }
}
