# Lumina Contracts

> Soroban smart contracts for Lumina, an open-source event indexer and GraphQL data layer for the Stellar network.

Part of the Lumina project, split across three repos:

- [lumina-frontend](https://github.com/Lumeeena/lumina-frontend) — Next.js explorer UI
- [lumina-backend](https://github.com/Lumeeena/lumina-backend) — indexer + GraphQL API + PostgreSQL schema
- [lumina-contracts](https://github.com/Lumeeena/lumina-contracts) — this repo

## Lumina Registry

`registry/` — an on-chain manifest of Soroban contracts registered for Lumina indexing. Any project can call `register_contract()` to add their contract; [lumina-backend](https://github.com/Lumeeena/lumina-backend)'s indexer can then discover and index their events.

```rust
registry.register_contract(owner, contract_id, "My Protocol", "A DeFi protocol on Stellar")
```

`get_active_contracts(offset, limit)` returns a paginated list of active registrations for discovery.

Registrations are also manageable after the fact:

| Method | Who can call it |
| --- | --- |
| `get_contracts_by_owner(owner, offset, limit)` | anyone — paginated, includes the owner's deactivated entries |
| `update_metadata(owner, contract_id, name, description)` | the registered owner only |
| `transfer_ownership(caller, contract_id, new_owner)` | the current owner or the admin |
| `deactivate(caller, contract_id)` | the current owner or the admin |

## Build & Test

```bash
cargo build --target wasm32-unknown-unknown --release
cargo test
```

## Deploying

Deployed on **testnet** at:

```
CAYUDQPV3RKPM3EXDFGI3457FV677JLUCJ4OLKWGCUBPRIHYKXK3WFAZ
```

[lumina-backend](https://github.com/Lumeeena/lumina-backend)'s indexer polls this contract for discovery when configured with `REGISTRY_CONTRACT_ID` (see that repo's README). See [DEPLOY.md](./DEPLOY.md) for the deployment steps used, and how to register your own contract.

## License

MIT
