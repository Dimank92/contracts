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
//! refunded the unstreamed remainder.

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env, Vec};

#[cfg(test)]
mod test;

mod error;
mod events;
pub mod math;
mod migrations;
mod types;

use error::Error;
pub use migrations::CURRENT_SCHEMA_VERSION;
use types::{DataKey, Schedule, Stream};

/// Maximum number of checkpoints a `Stepped` schedule may carry.
pub const MAX_CHECKPOINTS: u32 = 64;

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
    withdrawable_at(stream, now)
}

/// Pure function of the ledger timestamp and the schedule: the cumulative
/// amount vested as of `now`, in `[0, total]`. Never decreases with time.
fn withdrawable_at(stream: &Stream, now: u64) -> i128 {
    if stream.canceled || now >= stream.end {
        return stream.total;
    }
    match &stream.schedule {
        Schedule::Linear => linear_at(stream, now),
        Schedule::LinearWithCliff { cliff } => {
            if now < *cliff {
                0
            } else {
                linear_at(stream, now)
            }
        }
        Schedule::Stepped { checkpoints } => {
            let mut vested = 0i128;
            for (ts, cumulative) in checkpoints.iter() {
                if now >= ts {
                    vested = cumulative;
                } else {
                    break;
                }
            }
            vested
        }
    }
}

/// Continuous linear accrual from `start` (not from the cliff), floored.
fn linear_at(stream: &Stream, now: u64) -> i128 {
    if now <= stream.start {
        return 0;
    }
    math::mul_div_floor(
        stream.total,
        (now - stream.start) as i128,
        (stream.end - stream.start) as i128,
    )
}

/// Validates a schedule against the stream bounds and total, panicking with
/// a typed error on any violation.
fn validate_schedule(env: &Env, schedule: &Schedule, total: i128, start: u64, end: u64) {
    match schedule {
        Schedule::Linear => {}
        Schedule::LinearWithCliff { cliff } => {
            if *cliff < start || *cliff > end {
                panic_with_error!(env, Error::InvalidSchedule);
            }
        }
        Schedule::Stepped { checkpoints } => {
            if checkpoints.len() > MAX_CHECKPOINTS {
                panic_with_error!(env, Error::TooManyCheckpoints);
            }
            if checkpoints.is_empty() {
                panic_with_error!(env, Error::InvalidSchedule);
            }
            let mut prev_ts = start;
            let mut prev_cum = 0i128;
            for (ts, cumulative) in checkpoints.iter() {
                if ts <= prev_ts || cumulative <= prev_cum || ts > end {
                    panic_with_error!(env, Error::InvalidSchedule);
                }
                prev_ts = ts;
                prev_cum = cumulative;
            }
            if prev_cum != total {
                panic_with_error!(env, Error::InvalidSchedule);
            }
        }
    }
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
    /// (`start == end`) are rejected.
    ///
    /// Backward-compatible entrypoint: builds a `LinearWithCliff` schedule
    /// (or plain `Linear` when `cliff == start`).
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
    ) -> u64 {
        let schedule = if cliff == start {
            Schedule::Linear
        } else {
            Schedule::LinearWithCliff { cliff }
        };
        Self::create_stream_with_schedule(
            env, funder, recipient, token, total, start, end, schedule,
        )
    }

    /// Creates a stream with an explicit schedule variant, selected at
    /// creation time. The schedule is validated once here rather than on
    /// every read.
    #[allow(clippy::too_many_arguments)]
    pub fn create_stream_with_schedule(
        env: Env,
        funder: Address,
        recipient: Address,
        token: Address,
        total: i128,
        start: u64,
        end: u64,
        schedule: Schedule,
    ) -> u64 {
        funder.require_auth();
        migrations::require_current(&env);
        if total <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        if start >= end {
            panic_with_error!(&env, Error::InvalidSchedule);
        }
        validate_schedule(&env, &schedule, total, start, end);

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
                end,
                schedule,
                withdrawn: 0,
                canceled: false,
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
        events::withdrawn(&env, stream_id, stream.recipient, amount, &stream.schedule);
    }

    /// Funder or governance cancels. The recipient receives everything
    /// accrued but not yet withdrawn (zero if still before the cliff) and
    /// the funder is refunded the unstreamed remainder.
    pub fn cancel(env: Env, caller: Address, stream_id: u64) {
        caller.require_auth();
        migrations::require_current(&env);
        let mut stream = load(&env, stream_id);
        if caller != stream.funder && caller != Self::get_governance(env.clone()) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if stream.canceled {
            panic_with_error!(&env, Error::StreamCanceled);
        }

        let streamed = streamed_at(&stream, env.ledger().timestamp());
        let to_recipient = streamed - stream.withdrawn;
        let to_funder = stream.total - streamed;

        stream.total = streamed;
        stream.withdrawn = streamed;
        stream.canceled = true;
        save(&env, stream_id, &stream);

        let token = token::Client::new(&env, &stream.token);
        let this = env.current_contract_address();
        if to_recipient > 0 {
            token.transfer(&this, &stream.recipient, &to_recipient);
        }
        if to_funder > 0 {
            token.transfer(&this, &stream.funder, &to_funder);
        }
        events::canceled(&env, stream_id, to_recipient, to_funder);
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

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Option<(u32, u32)> {
        zenith_common::migrations::migration_state(&env)
    }

    /// `(recipient_withdrawable, funder_refundable)` as of now: what the
    /// recipient could withdraw right now, and what the funder would get
    /// back if the stream were cancelled right now.
    pub fn balance_of(env: Env, stream_id: u64) -> (i128, i128) {
        let stream = load(&env, stream_id);
        let streamed = streamed_at(&stream, env.ledger().timestamp());
        (streamed - stream.withdrawn, stream.total - streamed)
    }

    /// Cumulative amount vested as of now, per the stream's schedule. Pure
    /// function of the ledger timestamp and the schedule.
    pub fn withdrawable(env: Env, stream_id: u64) -> i128 {
        let stream = load(&env, stream_id);
        withdrawable_at(&stream, env.ledger().timestamp())
    }

    pub fn get_stream(env: Env, stream_id: u64) -> Option<Stream> {
        env.storage().persistent().get(&DataKey::Stream(stream_id))
    }

    pub fn get_stream_count(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::StreamCounter)
            .unwrap()
    }
}

/// Convenience constructor for a `Stepped` schedule from parallel arrays.
pub fn stepped_schedule(checkpoints: Vec<(u64, i128)>) -> Schedule {
    Schedule::Stepped { checkpoints }
}
