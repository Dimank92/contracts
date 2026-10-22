use soroban_sdk::{contracttype, Address};

/// Emitted when a stream is created.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamCreated {
    pub stream_id: u64,
    pub sender: Address,
    pub recipient: Address,
    pub deposit: i128,
    pub revocable: bool,
}

/// Emitted when a recipient withdraws vested-but-unwithdrawn funds.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Withdrawn {
    pub stream_id: u64,
    pub recipient: Address,
    pub amount: i128,
}

/// Emitted when a sender cancels a revocable stream.
///
/// `to_recipient` is the vested-but-unwithdrawn amount settled to the
/// recipient; `refunded` is the unvested remainder returned to the sender.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamCancelled {
    pub stream_id: u64,
    pub sender: Address,
    pub recipient: Address,
    pub to_recipient: i128,
    pub refunded: i128,
}

/// Emitted when an admin claws back a stream.
///
/// Behaves identically to cancellation but is emitted as a distinct event so
/// indexers can tell admin clawbacks apart from sender cancellations.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamClawedBack {
    pub stream_id: u64,
    pub admin: Address,
    pub sender: Address,
    pub recipient: Address,
    pub to_recipient: i128,
    pub refunded: i128,
}
