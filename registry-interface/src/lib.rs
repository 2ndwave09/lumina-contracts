// Copyright (c) Lumina contributors
// SPDX-License-Identifier: MIT
#no_std
cwarn(missing_docs)
//! Typed, read-only client for the Lumina Registry — for *contracts*, not
//! wallets.
//!
//! A Soroban contract that wants to ask "is this address listed, and is it
//! verified?" has two options today, and both are bad: hand-write
//! `env.invoke_contract(&system, symbol_short!("is_registered"), ...))` and
//! decode the `Val` yourself, or use `contractimport!` on the registry's wasm.
//! The second pulls the whole registry binary into your build, and the first
//! is unchecked at compile time — a renamed export becomes a runtime failure
//! in someone else's contract.
//!
//! This crate is the third option: a declared trait covering the registry's
//! read-only surface, and the [`RegistryInterfaceClient`] that
//! [scoroban_sdk::contractclient] generates from it.
//!
//! ```no_run
//! use lumina_registry_interface::RegistryInterfaceClient;
//! use soroban_sdk::{Address, Env};
//!
//! # fn check(env: &Env, registry: &Address, counterparty: &Address) {
//! let registry = RegistryInterfaceClient::new(env, registry);
//! if registry.is_registered(counterparty) && registry.is_verified(counterparty) {
//!     // ...
//! }
//! # }
//! ```
//!
//! ## Why the types are declared here instead of imported
//!
//! [`ContractEntry`], [`Category`], [`Reputation`] and friends are deliberately
//! *duplicated* from `lumina-registry` rather than re-exported from it. A
//! dependency edge on the contract crate would drag the registry's entire
//! `[contractimpl]` — every exported entrypoint and its spec — into every
//! consumer's wasm, which is both a size problem and a link problem: two
//! `[contractimpl]` exporting the same symbol do not coexist. `registry-v2`
//! does the same thing for the same reason, and says so at length.
//!
//! The duplication is a real risk — the two declarations could drift — so it
//! is *tested* rather than trusted. `tests/interface_matches_registry.rs` reads
//! the registry's compiled spec out of its wasm and asserts that every
//! function, type and error code declared here matches what the contract
//! actually exports. Run against a changed registry, it fails with the
//! signature that moved.
//!
//! ## The cost of a read
//!
//! A cross-contract read is **not** free, and not free in the way people
//! expect. It is not a `simulateTransaction` — a contract calling the registry
//! on-chain spends the transaction's whole resource budget, and the callee's
//! instructions and ledger reads are charged to *you*.
//!
//! Concretely, each read is one nested invocation frame, which costs:
//!
//! - a fixed instruction charge for the call itself, before the callee runs
//!   any code;
//! - every ledger entry the callee touches, at the callee's TTL — the registry
//!   stores registrations in `persistent` entries, so a read is a persistent
//!   entry read, which is the expensive kind;
//! - a fresh 1 MiB memory allocation for the callee's frame, and the memory
//!   cost of decoding the arguments you passed in and the result you get back.
//!
//! The practical consequence: **the number of calls is what you pay for.** Two
//! `is_*` calls cost strictly more than one `get_contract_profile` that returns
//! both facts, and a loop over counterparties multiplies the fixed per-call
//! charge every iteration. The `examples/registry-consumer` crate measures this
//! on the real registry wasm rather than estimating it — see its `cost` module
//! and the "What a cross-contract read costs" section of the README.

use soroban_sdk::{contractclient, contracterror, contracttype, Address, Env, String, Vec};

/// The read-only half of the Lumina Registry.
///
/// Every method here corresponds one-to-one to an export the registry contract
/// actually has, with the same name and the same arguments; nothing here mutates
/// state and nothing here requires authorization. A consumer that only
/// ever needs to *read* the registry should depend on this trait rather than
/// on the contract crate.
///
/// Methods are listed in the same order as the registry's own view section.
/// Two of them carry paging semantics that are easy to get wrong, and they are
/// called out on the methods themselves:
///
/// - `get_active_contracts`, `get_active_profiles`, `get_active_contract_ids`,
///   `get_active_contracts_page` and `get_active_profiles_page` treat `offset`
///   as a position in the *raw* index, not in the filtered result, so a page
///   can come back shorter than `limit` while more active entries follow.
///   The `_page` variants additionally return `has_more` so a caller can tell
///   "end of list" from "this page was short".
/// - `get_contracts_by_owner` includes deactivated entries, because an owner
///   listing is a management view, not a discovery one.
#[contractclient(name = "RegistryInterfaceClient")]
pub trait RegistryInterface {
    /// Which build of the registry is live at this address.
    fn get_version(env: Env) -> u32;

    /// The first admin address. Errors with `NotInitialized` before the
    /// registry has been set up.
    fn get_admin(env: Env) -> Result<Address, RegistryError>;

    /// The full current admin set. Errors with `NotInitialized` if empty.
    fn get_admins(env: Env) -> Result<Vec<Address>, RegistryError>;

    /// The number of approvals a proposal needs. Errors with `NotInitialized`
    /// before the registry has been set up.
    fn get_threshold(env: Env) -> Result<u32, RegistryError>;

    /// Retrieve a governance proposal by ID.
    fn get_proposal(env: Env, proposal_id: u32) -> Result<Proposal, RegistryError>;

    /// The categories a registration declared. Empty for a registration that
    /// predates the taxonomy, or for one that was never registered.
    fn get_categories(env: Env, contract_id: Address) -> Vec<Category>;

    /// Owner-set search tags for a registration. Empty for one that has none,
    /// or that was never registered.
    fn get_tags(env: Env, contract_id: Address) -> Vec<String>;

    /// One page of active registrations filed under `category`, in
    /// registration order.
    ///
    /// `offset` indexes the category's raw index rather than the filtered
    /// result, so a page can come back shorter than `limit` while more active
    /// registrations follow. See the trait docs.
    fn get_active_contracts_by_category(
        env: Env,
        category: Category,
        offset: u32,
        limit: u32,
    ) -> Vec<ContractEntry>;

    /// One page of active registrations filed under **any** of `categories` —
    /// the union, deduplicated, in registration order.
    ///
    /// Errors with `NoCategories` if `categories` is empty. Paging semantics
    /// as for `get_active_contracts_by_category`.
    fn get_active_by_categories(
        env: Env,
        categories: Vec<Category>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<ContractEntry>, RegistryError>;

    /// `(stake_token, treasury)`, or `StakingNotConfigured` /// governance has
    /// not opened staking yet.
    fn get_staking_config(env: Env) -> Result<(Address, Address), RegistryError>;

    /// The per-registration fee. Zero means registration is free.
    fn get_registration_fee(env: Env) -> i128;

    /// Currently staked balance. Zero for a registration that never staked,
    /// and zero — not an error — for an address that was never registered.
    fn get_stake(env: Env, contract_id: Address) -> i128;

    /// Whether governance has attested this registration. False, not an error,
    /// for an address that was never registered.
    fn is_verified(env: Env, contract_id: Address) -> bool;

    /// Whether `contract_id` has a registration at all, active or not.
    ///
    /// This is the cheapest question to ask the registry: one `has` against one
    /// persistent entry, no decoding. Prefer it whenever the answer is a
    /// yes/no gate and the details are not needed.
    fn is_registered(env: Env, contract_id: Address) -> bool;

    /// Aggregate counters: lifetime, active and verified totals, plus the
    /// staked count and amount. Maintained on write, so the read is cheap
    /// apart from the per-registration stake scan.
    fn get_registry_stats(env: Env) -> RegistryStats;

    /// Every slash ever levied against a registration, oldest first. Kept
    /// after deregistration so penalties stay auditable.
    fn get_slashes(env: Env, contract_id: Address) -> Vec<SlashRecord>;

    /// The full reputation signal for a registration. Returns zeroed values
    /// rather than erroring for an unregistered address, matching
    /// `is_registered`'s tolerance.
    fn get_reputation(env: Env, contract_id: Address) -> Reputation;

    /// A registration joined with its reputation — one call instead of
    /// `get_contract` plus `get_reputation`. Errors with `ContractNotFound`
    /// for an address that is not registered.
    ///
    /// **This is the one to reach for when you want both "listed" and
    /// "verified".** The two facts cost one nested invocation here versus two
    /// via `is_registered` + `is_verified`, and the fixed per-call charge is
    /// the part that dominates a cheap read.
    fn get_contract_profile(env: Env, contract_id: Address) -> Result<ContractProfile, RegistryError>;

    /// `get_active_contracts` with each entry's reputation attached.
    fn get_active_profiles(env: Env, offset: u32, limit: u32) -> Vec<ContractProfile>;

    /// The stored metadata entry for a registered contract. Errors with
    /// `ContractNotFound` if there is no registration.
    fn get_contract(env: Env, contract_id: Address) -> Result<ContractEntry, RegistryError>;

    /// Live registrations: deactivated included, deregistered excluded.
    fn get_contract_count(env: Env) -> u32;

    /// Lifetime registrations ever made. Never decremented, so it keeps
    /// counting across deregistration.
    fn get_total_registered(env: Env) -> u32;

    /// Currently listed (active) registrations. This is the figure a stats
    /// page wants.
    fn get_active_contract_count(env: Env) -> u32;

    /// One page of active registrations in registration order.
    ///
    /// `offset` indexes the raw index, so a page can come back shorter than
    /// `limit` while more active registrations follow. See the trait docs.
    fn get_active_contracts(env: Env, offset: u32, limit: u32) -> Vec<ContractEntry>;

    /// As `get_active_contracts`, but only the addresses. Cheaper to decode
    /// and much smaller to return, for a consumer that does not read the
    /// metadata.
    fn get_active_contract_ids(env: Env, offset: u32, limit: u32) -> Vec<Address>;

    /// As `get_active_contracts`, plus `has_more` so the caller can tell an
    /// exhausted index from a short page.
    fn get_active_contracts_page(env: Env, offset: u32, limit: u32) -> ContractPage;

    /// As `get_active_profiles`, plus `has_more`.
    fn get_active_profiles_page(env: Env, offset: u32, limit: u32) -> ContractProfilePage;

    /// Every contract registered by `owner`, **including** deactivated ones.
    fn get_contracts_by_owner(env: Env, owner: Address, offset: u32, limit: u32) -> Vec<ContractEntry>;
}

/// Errors the registry's read-only surface can return.
///
/// Declared in full, with the same discriminants as `lumina_registry::RegistryError`,
/// not just the handful a read can actually produce. A client decodes a
/// contract error by matching on the enum it was generated against, so a
/// variant that is missing here turns a well-defined error into an opaque
/// decode failure. `tests/interface_matches_registry.rs` pins the whole list
/// against the contract's spec, so the two cannot drift.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum RegistryError {
    /// Contract is already initialized.
    AlreadyInitialized = 1,
    /// Caller lacks authorization for this action.
    Unauthorized = 2,
    /// The contract has not been initialized.
    NotInitialized = 3,
    /// No registration exists for this address.
    ContractNotFound = 4,
    /// The address is already registered.
    AlreadyRegistered = 5,
    /// The category list was empty where at least one was required.
    NoCategories = 6,
    /// Staking has not been configured.
    StakingNotConfigured = 7,
    /// The proposal does not exist.
    ProposalNotFound = 8,
    /// The proposal has already been executed.
    ProposalAlreadyExecuted = 9,
    /// The proposal has not gathered enough approvals.
    InsufficientApprovals = 10,
    /// The proposal's timelock has not elapsed.
    TimelockNotElapsed = 11,
    /// The caller has already approved the proposal.
    AlreadyApproved = 12,
    /// The proposal action is not recognized.
    UnknownAction = 13,
    /// The address is not a governance admin.
    NotAdmin = 14,
    /// The admin set would be empty.
    NoAdmins = 15,
    /// The admin address is already in the set.
    AdminAlreadyExists = 16,
    /// The admin address is not in the set.
    AdminNotFound = 17,
    /// The admin set would fall below the minimum.
    TooFewAdmins = 18,
    /// The threshold is invalid for the current admin set.
    InvalidThreshold = 19,
    /// The contract is still active and cannot be deregistered.
    StillActive = 20,
    /// The contract still has stake attached.
    StakeStillPresent = 21,
    /// The contract has not been deactivated.
    NotDeactivated = 22,
    /// The contract has no stake to withdraw.
    NoStake = 23,
    /// The stake amount is invalid.
    InvalidStake = 24,
    /// The slash amount is invalid.
    InvalidSlash = 25,
    /// The category is not recognized.
    UnknownCategory = 26,
    /// The category index is full.
    CategoryIndexFull = 27,
    /// The global index is full.
    IndexFull = 28,
    /// The registration rate limit was exceeded.
    RateLimitExceeded = 29,
    /// The owner is not allowlisted.
    NotAllowlisted = 30,
    /// The registration fee could not be transferred.
    FeeTransferFailed = 31,
    /// The proposal data is malformed.
    InvalidProposalData = 32,
    /// The category migration cursor is out of range.
    InvalidCursor = 33,
    /// The category migration would overflow the target index.
    MigrationOverflow = 34,
}

/// A governance proposal.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Proposal {
    /// Unique proposal id.
    pub id: u32,
    /// Admin who proposed the action.
    pub proposer: Address,
    /// Action symbol.
    pub action: Symbol,
    /// Action payload.
    pub data: Bytes,
    /// Admins who have approved.
    pub approvals: Vec<Address>,
    /// Timestamp the proposal became ready, if it has.
    pub ready_at: u64,
    /// Whether the proposal has been executed.
    pub executed: bool,
}

/// The categories a registration can be filed under.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Category {
    /// Financial services.
    Finance = 1,
    /// Developer tooling.
    DevTools = 2,
    /// Infrastructure.
    Infrastructure = 3,
    /// Gaming.
    Gaming = 4,
    /// NFT marketplaces.
    Nft = 5,
    /// Governance.
    Governance = 6,
    /// Social.
    Social = 7,
    /// Other.
    Other = 8,
}

/// A stored registration entry.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractEntry {
    /// The registered contract address.
    pub contract_id: Address,
    /// The owner address.
    pub owner: Address,
    /// Human-readable name.
    pub name: String,
    /// Categories the registration is filed under.
    pub categories: Vec<Category>,
    /// Owner-set search tags.
    pub tags: Vec<String>,
    /// Whether the registration is active.
    pub active: bool,
    /// Whether governance has attested it.
    pub verified: bool,
    /// Currently staked amount.
    pub stake: i128,
}

/// A slash record.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlashRecord {
    /// The contract the slash was levied against.
    pub contract_id: Address,
    /// The amount slashed.
    pub amount: i128,
    /// The reason given.
    pub reason: String,
    /// When the slash was levied.
    pub timestamp: u64,
}

/// The reputation signal for a registration.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reputation {
    /// Reputation score.
    pub score: i64,
    /// Number of slashes levied.
    pub slashes_count: u32,
    /// Total amount slashed.
    pub total_slashed: i128,
    /// Whether governance has attested it.
    pub verified: bool,
}

/// A registration joined with its reputation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractProfile {
    /// The underlying registration.
    pub entry: ContractEntry,
    /// The reputation signal.
    pub reputation: Reputation,
}

/// Aggregate registry counters.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryStats {
    /// Lifetime registrations.
    pub total_registered: u32,
    /// Currently active registrations.
    pub active_contracts: u32,
    /// Currently verified registrations.
    pub verified_contracts: u32,
    /// Number of registrations with a nonzero stake.
    pub staked_count: u32,
    /// Total amount staked.
    pub total_staked: i128,
}

/// A page of registrations with a more-flag.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractPage {
    /// The entries in this page.
    pub entries: Vec<ContractEntry>,
    /// Whether more entries follow.
    pub has_more: bool,
}

/// A page of profiles with a more-flag.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractProfilePage {
    /// The profiles in this page.
    pub profiles: Vec<ContractProfile>,
    /// Whether more entries follow.
    pub has_more: bool,
}
