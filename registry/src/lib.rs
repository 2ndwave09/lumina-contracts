#![no_std]
//! Lumina Registry — on-chain contract registry for the Lumina indexer.
//!
//! Projects deploy their Soroban contracts and register them here so that
//! the Lumina indexer can discover and prioritize indexing their events.
//! This creates a permissionless, decentralized index manifest for Stellar.
//!
//! Flow:
//!   1. Project deploys a Soroban contract
//!   2. Project calls register_contract() on the Lumina Registry
//!   3. Lumina indexer polls the Registry for new entries and begins indexing
//!   4. Indexed events become queryable via the Lumina GraphQL API

use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror,
    Address, Env, Symbol, String, Vec,
};

// ─── Errors ────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum RegistryError {
    AlreadyInitialized  = 1,
    Unauthorized        = 2,
    AlreadyRegistered   = 3,
    ContractNotFound    = 4,
    InvalidMetadata     = 5,
}

// ─── Types ─────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub struct ContractEntry {
    /// The registered Soroban contract address.
    pub contract_id: Address,
    /// Owner/deployer who registered this contract.
    pub owner: Address,
    /// Human-readable name (e.g. "Quorum Governance").
    pub name: String,
    /// Short description of what the contract does.
    pub description: String,
    /// Ledger at which this contract was registered.
    pub registered_at: u32,
    /// Whether indexing is currently active for this contract.
    pub active: bool,
}

#[contracttype]
pub enum DataKey {
    Admin,
    ContractCount,
    Contract(Address),
    OwnerContracts(Address), // owner → Vec<Address>
}

// ─── Contract ──────────────────────────────────────────────────────────────

#[contract]
pub struct LuminaRegistry;

#[contractimpl]
impl LuminaRegistry {
    pub fn initialize(env: Env, admin: Address) -> Result<(), RegistryError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(RegistryError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::ContractCount, &0u32);
        Ok(())
    }

    /// Register a Soroban contract for Lumina indexing.
    /// Anyone can register — the owner must authorize the call.
    pub fn register_contract(
        env: Env,
        owner: Address,
        contract_id: Address,
        name: String,
        description: String,
    ) -> Result<(), RegistryError> {
        owner.require_auth();

        if env.storage().persistent().has(&DataKey::Contract(contract_id.clone())) {
            return Err(RegistryError::AlreadyRegistered);
        }

        let entry = ContractEntry {
            contract_id: contract_id.clone(),
            owner: owner.clone(),
            name: name.clone(),
            description,
            registered_at: env.ledger().sequence(),
            active: true,
        };

        env.storage().persistent().set(&DataKey::Contract(contract_id.clone()), &entry);

        let count: u32 = env.storage().instance().get(&DataKey::ContractCount).unwrap_or(0);
        env.storage().instance().set(&DataKey::ContractCount, &(count + 1));

        env.events().publish(
            (Symbol::new(&env, "contract_registered"),),
            (contract_id, owner, name),
        );

        Ok(())
    }

    /// Pause indexing for a contract. Only the owner or admin can call this.
    pub fn deactivate(env: Env, caller: Address, contract_id: Address) -> Result<(), RegistryError> {
        caller.require_auth();

        let mut entry: ContractEntry = env.storage().persistent()
            .get(&DataKey::Contract(contract_id.clone()))
            .ok_or(RegistryError::ContractNotFound)?;

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if caller != entry.owner && caller != admin {
            return Err(RegistryError::Unauthorized);
        }

        entry.active = false;
        env.storage().persistent().set(&DataKey::Contract(contract_id.clone()), &entry);

        env.events().publish(
            (Symbol::new(&env, "contract_deactivated"),),
            (contract_id, caller),
        );

        Ok(())
    }

    // ─── View ──────────────────────────────────────────────────────────────

    pub fn get_contract(env: Env, contract_id: Address) -> Result<ContractEntry, RegistryError> {
        env.storage().persistent()
            .get(&DataKey::Contract(contract_id))
            .ok_or(RegistryError::ContractNotFound)
    }

    pub fn get_contract_count(env: Env) -> u32 {
        env.storage().instance().get(&DataKey::ContractCount).unwrap_or(0)
    }

    pub fn is_registered(env: Env, contract_id: Address) -> bool {
        env.storage().persistent().has(&DataKey::Contract(contract_id))
    }

    // TODO: get_active_contracts(offset, limit) → Vec<ContractEntry>
    // TODO: get_contracts_by_owner(owner) → Vec<ContractEntry>
    // TODO: update_metadata(owner, contract_id, name, description)
}
