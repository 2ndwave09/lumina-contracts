# Deploying the Lumina Registry

Deploying the registry is optional — the rest of Lumina (indexer/GraphQL/frontend)
works without it. Deploy (or reuse the existing testnet deployment below) when
you want the indexer to discover contracts from a live on-chain manifest
instead of (or in addition to) a static `INDEXED_CONTRACT_IDS` list.

## Already deployed on testnet

```
Contract ID: CAYUDQPV3RKPM3EXDFGI3457FV677JLUCJ4OLKWGCUBPRIHYKXK3WFAZ
Admin:       GBWKFFXZ5CJESIHP2EOID5IOXMF472RO5XOJ36X475D5LJGI3AF5R5KY
```

It has one demo entry (itself), registered to verify indexer discovery
end-to-end. Point `lumina-backend` at it directly — see that repo's README
for the `REGISTRY_CONTRACT_ID` / `REGISTRY_READ_ACCOUNT` env vars — or deploy
your own following the steps below.

## Deploying your own

### Prerequisites

- [Stellar CLI](https://developers.stellar.org/docs/tools/stellar-cli) (`stellar`, formerly `soroban`)
- A funded testnet identity

```bash
stellar keys generate lumina-deployer --network testnet --fund
```

### Build and deploy

```bash
stellar contract build
stellar contract deploy \
  --wasm target/wasm32v1-none/release/lumina_registry.wasm \
  --source lumina-deployer \
  --network testnet \
  --alias lumina-registry
```

This prints the deployed contract's `C...` address — save it as `REGISTRY_CONTRACT_ID`.
If the deploy step fails with `HostError: Error(Storage, MissingValue)` /
"Wasm does not exist", that's just RPC propagation lag after the upload —
rerun the same `deploy` command a few seconds later; it skips re-uploading
and picks up from the create-contract step.

### Initialize

```bash
stellar contract invoke \
  --id lumina-registry \
  --source lumina-deployer \
  --network testnet \
  -- initialize --admin <your-address-G...>
```

### Register a contract for indexing

```bash
stellar contract invoke \
  --id lumina-registry \
  --source lumina-deployer \
  --network testnet \
  -- register_contract \
  --owner <owner-address-G...> \
  --contract_id <target-contract-C...> \
  --name "My Protocol" \
  --description "A DeFi protocol on Stellar"
```

### Verify discovery works

```bash
stellar contract invoke \
  --id lumina-registry \
  --source lumina-deployer \
  --network testnet \
  -- get_active_contracts --offset 0 --limit 10
```

### Manage your registration

List everything one address has registered (deactivated entries included):

```bash
stellar contract invoke \
  --id lumina-registry \
  --source lumina-deployer \
  --network testnet \
  -- get_contracts_by_owner \
  --owner <owner-address-G...> --offset 0 --limit 10
```

Correct a name or description — only the registered owner can do this:

```bash
stellar contract invoke \
  --id lumina-registry \
  --source lumina-deployer \
  --network testnet \
  -- update_metadata \
  --owner <owner-address-G...> \
  --contract_id <target-contract-C...> \
  --name "My Protocol" \
  --description "An updated description"
```

Hand the registration to a new key — callable by the current owner or the
admin:

```bash
stellar contract invoke \
  --id lumina-registry \
  --source lumina-deployer \
  --network testnet \
  -- transfer_ownership \
  --caller <current-owner-G...> \
  --contract_id <target-contract-C...> \
  --new_owner <new-owner-G...>
```
