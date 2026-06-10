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
#[derive(Clone, Debug, PartialEq)]
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

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    fn setup() -> (Env, LuminaRegistryClient<'static>, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(LuminaRegistry, ());
        let client = LuminaRegistryClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin);
        (env, client, admin)
    }

    fn register_sample(env: &Env, client: &LuminaRegistryClient) -> (Address, Address) {
        let owner = Address::generate(env);
        let target = Address::generate(env);
        let name = String::from_str(env, "Test Contract");
        let description = String::from_str(env, "A test contract");
        client.register_contract(&owner, &target, &name, &description);
        (owner, target)
    }

    #[test]
    fn initialize_sets_zero_count() {
        let (_, client, _) = setup();
        assert_eq!(client.get_contract_count(), 0);
    }

    #[test]
    fn initialize_twice_fails() {
        let (_, client, admin) = setup();
        let result = client.try_initialize(&admin);
        assert_eq!(result, Err(Ok(RegistryError::AlreadyInitialized)));
    }

    #[test]
    fn register_contract_succeeds() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);

        assert!(client.is_registered(&target));
        assert_eq!(client.get_contract_count(), 1);

        let entry = client.get_contract(&target);
        assert_eq!(entry.owner, owner);
        assert_eq!(entry.contract_id, target);
        assert!(entry.active);
    }

    #[test]
    fn register_contract_rejects_duplicate() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);

        let name = String::from_str(&env, "Test Contract");
        let description = String::from_str(&env, "A test contract");
        let result = client.try_register_contract(&owner, &target, &name, &description);
        assert_eq!(result, Err(Ok(RegistryError::AlreadyRegistered)));
    }

    #[test]
    fn deactivate_by_owner_succeeds() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);

        client.deactivate(&owner, &target);
        assert!(!client.get_contract(&target).active);
    }

    #[test]
    fn deactivate_by_admin_succeeds() {
        let (env, client, admin) = setup();
        let (_owner, target) = register_sample(&env, &client);

        client.deactivate(&admin, &target);
        assert!(!client.get_contract(&target).active);
    }

    #[test]
    fn deactivate_by_unrelated_caller_fails() {
        let (env, client, _admin) = setup();
        let (_owner, target) = register_sample(&env, &client);
        let stranger = Address::generate(&env);

        let result = client.try_deactivate(&stranger, &target);
        assert_eq!(result, Err(Ok(RegistryError::Unauthorized)));
    }

    #[test]
    fn get_contract_not_found_for_unknown_address() {
        let (env, client, _admin) = setup();
        let target = Address::generate(&env);
        let result = client.try_get_contract(&target);
        assert_eq!(result, Err(Ok(RegistryError::ContractNotFound)));
    }

    #[test]
    fn is_registered_false_for_unknown_contract() {
        let (env, client, _admin) = setup();
        let target = Address::generate(&env);
        assert!(!client.is_registered(&target));
    }
}
