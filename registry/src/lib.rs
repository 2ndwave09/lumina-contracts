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
    /// Caller is not the registered owner of the contract. Distinct from
    /// `Unauthorized`, which also covers calls an admin would have been
    /// allowed to make.
    NotOwner            = 6,
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
    AllContracts,            // insertion-ordered Vec<Address> of every registered contract
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

        let mut owned = Self::owner_index(&env, &owner);
        owned.push_back(contract_id.clone());
        Self::set_owner_index(&env, &owner, &owned);

        let mut all: Vec<Address> = env.storage().instance().get(&DataKey::AllContracts).unwrap_or(Vec::new(&env));
        all.push_back(contract_id.clone());
        env.storage().instance().set(&DataKey::AllContracts, &all);

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

    /// Paginated list of active (non-deactivated) registered contracts, in
    /// registration order. Used by the Lumina indexer to discover what to index.
    pub fn get_active_contracts(env: Env, offset: u32, limit: u32) -> Vec<ContractEntry> {
        let all: Vec<Address> = env.storage().instance().get(&DataKey::AllContracts).unwrap_or(Vec::new(&env));
        let mut result = Vec::new(&env);

        let mut i = offset;
        while i < all.len() && result.len() < limit {
            let contract_id = all.get(i).unwrap();
            if let Some(entry) = env.storage().persistent().get::<DataKey, ContractEntry>(&DataKey::Contract(contract_id)) {
                if entry.active {
                    result.push_back(entry);
                }
            }
            i += 1;
        }

        result
    }

    /// Paginated list of every contract registered by `owner`, in registration
    /// order. Unlike `get_active_contracts` this does **not** filter on
    /// `active` — an owner should still be able to see (and manage) entries
    /// they have deactivated.
    pub fn get_contracts_by_owner(env: Env, owner: Address, offset: u32, limit: u32) -> Vec<ContractEntry> {
        let owned = Self::owner_index(&env, &owner);
        let mut result = Vec::new(&env);

        let mut i = offset;
        while i < owned.len() && result.len() < limit {
            let contract_id = owned.get(i).unwrap();
            if let Some(entry) = env.storage().persistent().get::<DataKey, ContractEntry>(&DataKey::Contract(contract_id)) {
                result.push_back(entry);
            }
            i += 1;
        }

        result
    }

    /// Update a registered contract's name and description.
    /// Only the current registered owner can call this — unlike `deactivate`,
    /// the admin has no say over how a project describes itself.
    pub fn update_metadata(
        env: Env,
        owner: Address,
        contract_id: Address,
        name: String,
        description: String,
    ) -> Result<(), RegistryError> {
        owner.require_auth();

        let mut entry: ContractEntry = env.storage().persistent()
            .get(&DataKey::Contract(contract_id.clone()))
            .ok_or(RegistryError::ContractNotFound)?;

        if owner != entry.owner {
            return Err(RegistryError::NotOwner);
        }

        entry.name = name.clone();
        entry.description = description;
        env.storage().persistent().set(&DataKey::Contract(contract_id.clone()), &entry);

        env.events().publish(
            (Symbol::new(&env, "metadata_updated"),),
            (contract_id, owner, name),
        );

        Ok(())
    }

    /// Hand a registration over to a new owner — e.g. when a project's
    /// deployer key rotates. Only the current owner or the admin can call
    /// this, mirroring `deactivate`'s authorization.
    ///
    /// Transferring to the current owner is a no-op, so the owner index is
    /// never left holding a duplicate entry.
    pub fn transfer_ownership(
        env: Env,
        caller: Address,
        contract_id: Address,
        new_owner: Address,
    ) -> Result<(), RegistryError> {
        caller.require_auth();

        let mut entry: ContractEntry = env.storage().persistent()
            .get(&DataKey::Contract(contract_id.clone()))
            .ok_or(RegistryError::ContractNotFound)?;

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if caller != entry.owner && caller != admin {
            return Err(RegistryError::Unauthorized);
        }

        let previous_owner = entry.owner.clone();
        if previous_owner == new_owner {
            return Ok(());
        }

        let mut previous_owned = Self::owner_index(&env, &previous_owner);
        if let Some(i) = previous_owned.first_index_of(&contract_id) {
            previous_owned.remove(i);
            Self::set_owner_index(&env, &previous_owner, &previous_owned);
        }

        let mut new_owned = Self::owner_index(&env, &new_owner);
        new_owned.push_back(contract_id.clone());
        Self::set_owner_index(&env, &new_owner, &new_owned);

        entry.owner = new_owner.clone();
        env.storage().persistent().set(&DataKey::Contract(contract_id.clone()), &entry);

        env.events().publish(
            (Symbol::new(&env, "ownership_transferred"),),
            (contract_id, previous_owner, new_owner),
        );

        Ok(())
    }
}

// ─── Internal helpers ──────────────────────────────────────────────────────
//
// Deliberately outside the `#[contractimpl]` block so they stay off the
// contract's exported interface.
impl LuminaRegistry {
    /// The `OwnerContracts` index lives in persistent storage (like the
    /// entries themselves) rather than instance storage, since it grows with
    /// the number of owners rather than being a single bounded value.
    fn owner_index(env: &Env, owner: &Address) -> Vec<Address> {
        env.storage().persistent()
            .get(&DataKey::OwnerContracts(owner.clone()))
            .unwrap_or(Vec::new(env))
    }

    fn set_owner_index(env: &Env, owner: &Address, contracts: &Vec<Address>) {
        env.storage().persistent().set(&DataKey::OwnerContracts(owner.clone()), contracts);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::{Address as _, MockAuth, MockAuthInvoke};
    use soroban_sdk::IntoVal;

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

    /// Register a fresh contract under an existing owner, so a single owner
    /// can end up holding several entries.
    fn register_for(env: &Env, client: &LuminaRegistryClient, owner: &Address) -> Address {
        let target = Address::generate(env);
        let name = String::from_str(env, "Test Contract");
        let description = String::from_str(env, "A test contract");
        client.register_contract(owner, &target, &name, &description);
        target
    }

    /// Whether a `get_contracts_by_owner` page contains a given contract.
    fn page_contains(entries: &Vec<ContractEntry>, contract_id: &Address) -> bool {
        entries.iter().any(|e| &e.contract_id == contract_id)
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

    #[test]
    fn get_active_contracts_empty_registry_returns_empty() {
        let (_, client, _admin) = setup();
        let result = client.get_active_contracts(&0, &10);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn get_active_contracts_returns_registered_entries() {
        let (env, client, _admin) = setup();
        let (_owner, target) = register_sample(&env, &client);

        let result = client.get_active_contracts(&0, &10);
        assert_eq!(result.len(), 1);
        assert_eq!(result.get(0).unwrap().contract_id, target);
    }

    #[test]
    fn get_active_contracts_excludes_deactivated() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);
        client.deactivate(&owner, &target);

        let result = client.get_active_contracts(&0, &10);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn get_active_contracts_respects_limit_and_offset() {
        let (env, client, _admin) = setup();
        for _ in 0..5 {
            register_sample(&env, &client);
        }

        let first_page = client.get_active_contracts(&0, &2);
        assert_eq!(first_page.len(), 2);

        let second_page = client.get_active_contracts(&2, &2);
        assert_eq!(second_page.len(), 2);

        let third_page = client.get_active_contracts(&4, &2);
        assert_eq!(third_page.len(), 1);
    }

    // ─── get_contracts_by_owner ────────────────────────────────────────────

    #[test]
    fn get_contracts_by_owner_empty_for_owner_with_none() {
        let (env, client, _admin) = setup();
        let stranger = Address::generate(&env);

        assert_eq!(client.get_contracts_by_owner(&stranger, &0, &10).len(), 0);
    }

    #[test]
    fn get_contracts_by_owner_empty_when_owner_has_none_but_others_do() {
        let (env, client, _admin) = setup();
        register_sample(&env, &client);
        let stranger = Address::generate(&env);

        assert_eq!(client.get_contracts_by_owner(&stranger, &0, &10).len(), 0);
    }

    #[test]
    fn get_contracts_by_owner_returns_single_entry() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);

        let entries = client.get_contracts_by_owner(&owner, &0, &10);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries.get(0).unwrap().contract_id, target);
        assert_eq!(entries.get(0).unwrap().owner, owner);
    }

    #[test]
    fn get_contracts_by_owner_returns_only_that_owners_contracts() {
        let (env, client, _admin) = setup();
        let (owner_a, target_a) = register_sample(&env, &client);
        let (owner_b, target_b) = register_sample(&env, &client);
        let target_a2 = register_for(&env, &client, &owner_a);

        let a_entries = client.get_contracts_by_owner(&owner_a, &0, &10);
        assert_eq!(a_entries.len(), 2);
        assert!(page_contains(&a_entries, &target_a));
        assert!(page_contains(&a_entries, &target_a2));
        assert!(!page_contains(&a_entries, &target_b));

        let b_entries = client.get_contracts_by_owner(&owner_b, &0, &10);
        assert_eq!(b_entries.len(), 1);
        assert!(page_contains(&b_entries, &target_b));
    }

    #[test]
    fn get_contracts_by_owner_respects_limit_and_offset() {
        let (env, client, _admin) = setup();
        let (owner, _first) = register_sample(&env, &client);
        for _ in 0..4 {
            register_for(&env, &client, &owner);
        }
        // A second owner's entries must not leak into the pages above.
        register_sample(&env, &client);

        assert_eq!(client.get_contracts_by_owner(&owner, &0, &10).len(), 5);
        assert_eq!(client.get_contracts_by_owner(&owner, &0, &2).len(), 2);
        assert_eq!(client.get_contracts_by_owner(&owner, &2, &2).len(), 2);
        assert_eq!(client.get_contracts_by_owner(&owner, &4, &2).len(), 1);
        assert_eq!(client.get_contracts_by_owner(&owner, &5, &2).len(), 0);
        assert_eq!(client.get_contracts_by_owner(&owner, &99, &2).len(), 0);
    }

    #[test]
    fn get_contracts_by_owner_pages_are_in_registration_order() {
        let (env, client, _admin) = setup();
        let (owner, first) = register_sample(&env, &client);
        let second = register_for(&env, &client, &owner);
        let third = register_for(&env, &client, &owner);

        assert_eq!(client.get_contracts_by_owner(&owner, &0, &1).get(0).unwrap().contract_id, first);
        assert_eq!(client.get_contracts_by_owner(&owner, &1, &1).get(0).unwrap().contract_id, second);
        assert_eq!(client.get_contracts_by_owner(&owner, &2, &1).get(0).unwrap().contract_id, third);
    }

    #[test]
    fn get_contracts_by_owner_includes_deactivated_entries() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);
        client.deactivate(&owner, &target);

        // `get_active_contracts` filters these out; an owner still needs to
        // see their own deactivated registrations.
        assert_eq!(client.get_active_contracts(&0, &10).len(), 0);
        let entries = client.get_contracts_by_owner(&owner, &0, &10);
        assert_eq!(entries.len(), 1);
        assert!(!entries.get(0).unwrap().active);
    }

    // ─── update_metadata ───────────────────────────────────────────────────

    #[test]
    fn update_metadata_by_owner_succeeds() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);

        let name = String::from_str(&env, "Renamed Protocol");
        let description = String::from_str(&env, "A corrected description");
        client.update_metadata(&owner, &target, &name, &description);

        let entry = client.get_contract(&target);
        assert_eq!(entry.name, name);
        assert_eq!(entry.description, description);
        // Everything else is untouched.
        assert_eq!(entry.owner, owner);
        assert_eq!(entry.contract_id, target);
        assert!(entry.active);
    }

    #[test]
    fn update_metadata_rejects_non_owner_caller() {
        let (env, client, _admin) = setup();
        let (_owner, target) = register_sample(&env, &client);
        let stranger = Address::generate(&env);

        let name = String::from_str(&env, "Hijacked");
        let description = String::from_str(&env, "Hijacked");
        let result = client.try_update_metadata(&stranger, &target, &name, &description);
        assert_eq!(result, Err(Ok(RegistryError::NotOwner)));
    }

    #[test]
    fn update_metadata_rejects_admin_caller() {
        let (env, client, admin) = setup();
        let (_owner, target) = register_sample(&env, &client);

        // Unlike `deactivate`, the admin is not privileged here.
        let name = String::from_str(&env, "Admin Rename");
        let description = String::from_str(&env, "Admin Rename");
        let result = client.try_update_metadata(&admin, &target, &name, &description);
        assert_eq!(result, Err(Ok(RegistryError::NotOwner)));
    }

    #[test]
    fn update_metadata_rejects_unregistered_contract() {
        let (env, client, _admin) = setup();
        let owner = Address::generate(&env);
        let unregistered = Address::generate(&env);

        let name = String::from_str(&env, "Nothing");
        let description = String::from_str(&env, "Nothing");
        let result = client.try_update_metadata(&owner, &unregistered, &name, &description);
        assert_eq!(result, Err(Ok(RegistryError::ContractNotFound)));
    }

    #[test]
    fn update_metadata_succeeds_with_a_real_owner_signature() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);

        let name = String::from_str(&env, "Signed Rename");
        let description = String::from_str(&env, "Signed Rename");

        // Replaces `mock_all_auths` — only this exact invocation, signed by
        // the owner, is authorized.
        env.mock_auths(&[MockAuth {
            address: &owner,
            invoke: &MockAuthInvoke {
                contract: &client.address,
                fn_name: "update_metadata",
                args: (owner.clone(), target.clone(), name.clone(), description.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.update_metadata(&owner, &target, &name, &description);
        assert_eq!(client.get_contract(&target).name, name);
    }

    #[test]
    #[should_panic(expected = "Error(Auth, InvalidAction)")]
    fn update_metadata_without_owner_signature_panics() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);
        let stranger = Address::generate(&env);

        let name = String::from_str(&env, "Unsigned Rename");
        let description = String::from_str(&env, "Unsigned Rename");

        // Someone else signs an invocation that names `owner` as the caller.
        // `owner.require_auth()` must reject it, rather than the contract
        // merely comparing addresses.
        env.mock_auths(&[MockAuth {
            address: &stranger,
            invoke: &MockAuthInvoke {
                contract: &client.address,
                fn_name: "update_metadata",
                args: (owner.clone(), target.clone(), name.clone(), description.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.update_metadata(&owner, &target, &name, &description);
    }

    // ─── transfer_ownership ────────────────────────────────────────────────

    #[test]
    fn transfer_ownership_moves_entry_between_owner_indices() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);
        let kept = register_for(&env, &client, &owner);
        let new_owner = Address::generate(&env);

        client.transfer_ownership(&owner, &target, &new_owner);

        let old_entries = client.get_contracts_by_owner(&owner, &0, &10);
        assert_eq!(old_entries.len(), 1);
        assert!(page_contains(&old_entries, &kept));
        assert!(!page_contains(&old_entries, &target));

        let new_entries = client.get_contracts_by_owner(&new_owner, &0, &10);
        assert_eq!(new_entries.len(), 1);
        assert!(page_contains(&new_entries, &target));

        assert_eq!(client.get_contract(&target).owner, new_owner);
    }

    #[test]
    fn transfer_ownership_appends_to_an_existing_new_owner_index() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);
        let (new_owner, existing) = register_sample(&env, &client);

        client.transfer_ownership(&owner, &target, &new_owner);

        assert_eq!(client.get_contracts_by_owner(&owner, &0, &10).len(), 0);
        let new_entries = client.get_contracts_by_owner(&new_owner, &0, &10);
        assert_eq!(new_entries.len(), 2);
        assert!(page_contains(&new_entries, &existing));
        assert!(page_contains(&new_entries, &target));
    }

    #[test]
    fn transfer_ownership_by_admin_succeeds() {
        let (env, client, admin) = setup();
        let (owner, target) = register_sample(&env, &client);
        let new_owner = Address::generate(&env);

        client.transfer_ownership(&admin, &target, &new_owner);

        assert_eq!(client.get_contract(&target).owner, new_owner);
        assert_eq!(client.get_contracts_by_owner(&owner, &0, &10).len(), 0);
        assert_eq!(client.get_contracts_by_owner(&new_owner, &0, &10).len(), 1);
    }

    #[test]
    fn transfer_ownership_by_unrelated_caller_fails() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);
        let stranger = Address::generate(&env);
        let new_owner = Address::generate(&env);

        let result = client.try_transfer_ownership(&stranger, &target, &new_owner);
        assert_eq!(result, Err(Ok(RegistryError::Unauthorized)));

        // Nothing moved.
        assert_eq!(client.get_contract(&target).owner, owner);
        assert_eq!(client.get_contracts_by_owner(&owner, &0, &10).len(), 1);
        assert_eq!(client.get_contracts_by_owner(&new_owner, &0, &10).len(), 0);
    }

    #[test]
    fn transfer_ownership_rejects_unregistered_contract() {
        let (env, client, _admin) = setup();
        let caller = Address::generate(&env);
        let unregistered = Address::generate(&env);
        let new_owner = Address::generate(&env);

        let result = client.try_transfer_ownership(&caller, &unregistered, &new_owner);
        assert_eq!(result, Err(Ok(RegistryError::ContractNotFound)));
    }

    #[test]
    fn transfer_ownership_to_the_current_owner_is_a_no_op() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);

        client.transfer_ownership(&owner, &target, &owner);

        // Crucially, the index must not now hold the contract twice.
        let entries = client.get_contracts_by_owner(&owner, &0, &10);
        assert_eq!(entries.len(), 1);
        assert_eq!(client.get_contract(&target).owner, owner);
    }

    #[test]
    fn transfer_ownership_hands_over_owner_rights() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);
        let new_owner = Address::generate(&env);

        client.transfer_ownership(&owner, &target, &new_owner);

        // The previous owner has lost both privileges...
        let name = String::from_str(&env, "Ex-owner Rename");
        assert_eq!(
            client.try_update_metadata(&owner, &target, &name, &name),
            Err(Ok(RegistryError::NotOwner)),
        );
        assert_eq!(
            client.try_deactivate(&owner, &target),
            Err(Ok(RegistryError::Unauthorized)),
        );

        // ...and the new owner has gained them.
        client.update_metadata(&new_owner, &target, &name, &name);
        assert_eq!(client.get_contract(&target).name, name);
        client.deactivate(&new_owner, &target);
        assert!(!client.get_contract(&target).active);
    }

    #[test]
    fn admin_can_still_deactivate_after_ownership_transfer() {
        let (env, client, admin) = setup();
        let (owner, target) = register_sample(&env, &client);
        let new_owner = Address::generate(&env);

        client.transfer_ownership(&owner, &target, &new_owner);
        client.deactivate(&admin, &target);

        assert!(!client.get_contract(&target).active);
    }

    #[test]
    fn transfer_ownership_succeeds_with_a_real_owner_signature() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);
        let new_owner = Address::generate(&env);

        env.mock_auths(&[MockAuth {
            address: &owner,
            invoke: &MockAuthInvoke {
                contract: &client.address,
                fn_name: "transfer_ownership",
                args: (owner.clone(), target.clone(), new_owner.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.transfer_ownership(&owner, &target, &new_owner);
        assert_eq!(client.get_contract(&target).owner, new_owner);
    }

    #[test]
    #[should_panic(expected = "Error(Auth, InvalidAction)")]
    fn transfer_ownership_without_caller_signature_panics() {
        let (env, client, _admin) = setup();
        let (owner, target) = register_sample(&env, &client);
        let new_owner = Address::generate(&env);

        env.mock_auths(&[MockAuth {
            address: &new_owner,
            invoke: &MockAuthInvoke {
                contract: &client.address,
                fn_name: "transfer_ownership",
                args: (owner.clone(), target.clone(), new_owner.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.transfer_ownership(&owner, &target, &new_owner);
    }

    #[test]
    fn register_contract_populates_the_owner_index() {
        let (env, client, _admin) = setup();
        let owner = Address::generate(&env);

        assert_eq!(client.get_contracts_by_owner(&owner, &0, &10).len(), 0);
        let target = register_for(&env, &client, &owner);
        let entries = client.get_contracts_by_owner(&owner, &0, &10);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries.get(0).unwrap().contract_id, target);
    }
}
