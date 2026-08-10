# Deploying the Lumina Registry

This is a manual, one-time step — the registry doesn't need to be deployed
for the rest of Lumina (indexer/GraphQL/frontend) to work. Deploy it once
you want the indexer to discover contracts from a live on-chain manifest
instead of (or in addition to) a static `INDEXED_CONTRACT_IDS` list.

## Prerequisites

- [Stellar CLI](https://developers.stellar.org/docs/tools/stellar-cli) (`stellar`, formerly `soroban`)
- A funded testnet identity

```bash
stellar keys generate lumina-deployer --network testnet --fund
```

## Build and deploy

```bash
cd contracts
stellar contract build
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/lumina_registry.wasm \
  --source lumina-deployer \
  --network testnet
```

This prints the deployed contract's `C...` address — save it as `REGISTRY_CONTRACT_ID`.

## Initialize

```bash
stellar contract invoke \
  --id <REGISTRY_CONTRACT_ID> \
  --source lumina-deployer \
  --network testnet \
  -- initialize --admin <your-address-G...>
```

## Register a contract for indexing

```bash
stellar contract invoke \
  --id <REGISTRY_CONTRACT_ID> \
  --source lumina-deployer \
  --network testnet \
  -- register_contract \
  --owner <owner-address-G...> \
  --contract_id <target-contract-C...> \
  --name "My Protocol" \
  --description "A DeFi protocol on Stellar"
```

## What's next

The indexer's Soroban event indexing (`SOROBAN_RPC_URL` + `INDEXED_CONTRACT_IDS`,
see `indexer/src/index.ts`) currently takes a static, manually-curated contract
ID list. `get_active_contracts(offset, limit)` on the deployed registry is
ready to be polled to populate that list automatically — that wiring
(indexer → Soroban RPC `simulateTransaction` → registry → merge into the
indexed set) is the one piece intentionally left for a follow-up, since it
needs a real deployed registry to develop and verify against.
