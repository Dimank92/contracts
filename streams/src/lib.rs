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
use types::{DataKey, Stream, StreamStatus};

#[contract]
pub struct Streams;

/// Hard cap on the number of streams returned by a single enumeration
/// page. Callers paginate with the returned cursor; no query ever returns
/// more than this many entries.
const MAX_PAGE_SIZE: u32 = 50;

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

/// Vested-but-unwithdrawn amount for `stream` as of `now`. This is the
/// single source of truth shared by `withdraw` and `withdrawable`.
fn withdrawable_at(stream: &Stream, now: u64) -> i128 {
    streamed_at(stream, now) - stream.withdrawn
}

/// Terminal status of a stream: cancelled, fully withdrawn, or live.
fn status_of(stream: &Stream) -> StreamStatus {
    if stream.canceled {
        StreamStatus::Canceled
    } else if stream.withdrawn >= stream.total {
        StreamStatus::Completed
    } else {
        StreamStatus::Active
    }
}

/// Appends `stream_id` to the sender/recipient index for `stream` and
/// extends the index entry's TTL so it cannot expire before the stream.
fn index_stream(env: &Env, stream_id: u64, stream: &Stream) {
    let mut sender_ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&DataKey::SenderStreams(stream.funder.clone()))
        .unwrap_or_else(|| Vec::new(env));
    sender_ids.push_back(stream_id);
    env.storage()
        .persistent()
        .set(&DataKey::SenderStreams(stream.funder.clone()), &sender_ids);

    let mut recipient_ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&DataKey::RecipientStreams(stream.recipient.clone()))
        .unwrap_or_else(|| Vec::new(env));
    recipient_ids.push_back(stream_id);
    env.storage()
        .persistent()
        .set(&DataKey::RecipientStreams(stream.recipient.clone()), &recipient_ids);
}

/// Removes `stream_id` from the recipient index for `old` and adds it to
/// the index for `new`, keeping enumeration in sync with the record.
fn reindex_recipient(env: &Env, stream_id: u64, old: &Address, new: &Address) {
    let mut old_ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&DataKey::RecipientStreams(old.clone()))
        .unwrap_or_else(|| Vec::new(env));
    let mut kept: Vec<u64> = Vec::new(env);
    for id in old_ids.iter() {
        if id != stream_id {
            kept.push_back(id);
        }
    }
    old_ids = kept;
    env.storage()
        .persistent()
        .set(&DataKey::RecipientStreams(old.clone()), &old_ids);

    let mut new_ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&DataKey::RecipientStreams(new.clone()))
        .unwrap_or_else(|| Vec::new(env));
    new_ids.push_back(stream_id);
    env.storage()
        .persistent()
        .set(&DataKey::RecipientStreams(new.clone()), &new_ids);
}

/// Returns up to `limit` (capped at `MAX_PAGE_SIZE`) ids from `ids`
/// starting at `cursor`, plus the next cursor (or `None` when exhausted).
fn paginate(env: &Env, ids: &Vec<u64>, cursor: u32, limit: u32) -> (Vec<u64>, Option<u32>) {
    let cap = if limit == 0 || limit > MAX_PAGE_SIZE {
        MAX_PAGE_SIZE
    } else {
        limit
    };
    let len = ids.len();
    let mut page: Vec<u64> = Vec::new(env);
    let mut i = cursor;
    while i < len && page.len() < cap {
        page.push_back(ids.get(i).unwrap());
        i += 1;
    }
    let next = if i < len { Some(i) } else { None };
    (page, next)
}

#[contractimpl]
impl Streams {
    pub fn initialize(env: Env, governance: Address) {
        zenith_common::require_uninitialized(
            &env,
            &DataKey::Initialized,
            &DataKey::Governance,
            Error::AlreadyInitialized,
        );
        zenith_common::mark_initialized(&env, &DataKey::Initialized);
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

        let stream = Stream {
            funder: funder.clone(),
            recipient: recipient.clone(),
            token,
            total,
            start,
            cliff,
            end,
            withdrawn: 0,
            canceled: false,
        };
        save(&env, stream_id, &stream);
        index_stream(&env, stream_id, &stream);
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
        let available = withdrawable_at(&stream, env.ledger().timestamp());
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
        reindex_recipient(&env, stream_id, &old, &new_recipient);
        events::transferred(&env, stream_id, old, new_recipient);
    }

    // ── Read-only introspection ─────────────────────────────────────────────

    /// Returns the full stream record for `stream_id`. Cancelled and
    /// fully-withdrawn streams remain queryable; an unknown id panics with
    /// `Error::StreamNotFound`.
    pub fn get_stream(env: Env, stream_id: u64) -> Stream {
        load(&env, stream_id)
    }

    /// Returns the vested-but-unwithdrawn amount for `stream_id` as of the
    /// current ledger timestamp. Uses the same computation as `withdraw`.
    pub fn withdrawable(env: Env, stream_id: u64) -> i128 {
        let stream = load(&env, stream_id);
        withdrawable_at(&stream, env.ledger().timestamp())
    }

    /// Returns the terminal/live status of `stream_id`.
    pub fn get_stream_status(env: Env, stream_id: u64) -> StreamStatus {
        status_of(&load(&env, stream_id))
    }

    /// Cursor-paginated enumeration of streams funded by `address`.
    /// Returns at most `MAX_PAGE_SIZE` ids and the next cursor (`None` when
    /// exhausted).
    pub fn get_sender_streams(
        env: Env,
        address: Address,
        cursor: u32,
        limit: u32,
    ) -> (Vec<u64>, Option<u32>) {
        let ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::SenderStreams(address))
            .unwrap_or_else(|| Vec::new(&env));
        paginate(&env, &ids, cursor, limit)
    }

    /// Cursor-paginated enumeration of streams paying `address`.
    /// Returns at most `MAX_PAGE_SIZE` ids and the next cursor (`None` when
    /// exhausted).
    pub fn get_recipient_streams(
        env: Env,
        address: Address,
        cursor: u32,
        limit: u32,
    ) -> (Vec<u64>, Option<u32>) {
        let ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::RecipientStreams(address))
            .unwrap_or_else(|| Vec::new(&env));
        paginate(&env, &ids, cursor, limit)
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
