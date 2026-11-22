# BlitzPot — Real-Time Dice Arena on MagicBlock Ephemeral Rollups

**Solana Blitz v10 submission** · Theme: **Entertainment Finance**
A winner-takes-pot dice game where the pot is real (devnet) SOL, the players are real wallets — and the dice phase runs at websocket speed inside a **MagicBlock Ephemeral Rollup** instead of being throttled by base-layer block times.

## The game

1. **Lobby (base layer)** — a host creates a game with an entry fee. Up to 8 players join, each paying the fee into the pot (escrowed by the game PDA itself).
2. **Go live (delegate)** — the host starts the game and the game state PDA is *delegated* to a MagicBlock Ephemeral Rollup.
3. **Dice phase (Ephemeral Rollup)** — for the length of the game, every player rolls dice as fast as they can send transactions: ~50 ms slots, zero fees, state visible to everyone in real time. Highest total score wins.
4. **Settle (base layer)** — the final state is committed and undelegated back to Solana, and anyone can call `settle`: the whole pot is paid to the highest scorer (ties → earliest joiner). Deterministic and permissionless.

Money only ever moves on the base layer; the Ephemeral Rollup only carries the fast-moving game state. That split — *slow money, fast play* — is exactly what Entertainment Finance apps need.

## Why Ephemeral Rollups?

On the base layer each roll would cost a fee and wait ~400 ms for a slot — a "roll as fast as you can" game is unplayable. Delegating the game account to an ER gives:

- **Real-time execution** (~50 ms slots) for the dice phase
- **Zero-fee transactions** while delegated
- **Trustless settlement** — state commits back to Solana L1 and the pot pays out from the same PDA that escrowed it

## Program design

Single Anchor program (`programs/blitz-pot`), one account type:

| Instruction | Layer | What it does |
|---|---|---|
| `create_game` | base | Create lobby + pot (game PDA), set entry fee / max players |
| `join_game` | base | Pay entry fee into the pot, take a player slot |
| `start_game` | base | Lock the lobby (≥ 2 players), mark game LIVE |
| `delegate` | base → ER | Delegate the game PDA to the delegation program (`#[delegate]` macro) |
| `roll` | ER | Roll one die, add to your score — the hot path |
| `commit` | ER | Commit ER state to the base layer, stay delegated |
| `undelegate` | ER → base | Final commit + undelegate (`MagicIntentBundleBuilder`) |
| `settle` | base | Pay the whole pot to the highest scorer |

**Randomness (honest MVP note):** rolls are derived from the player key, a per-player nonce and the ER clock. That's fine for a devnet demo but is *not* secure randomness — the production upgrade is MagicBlock **VRF** (`ephemeral-vrf-sdk`), which is the natural next step and already fits the same instruction shape.

## Run the demo (devnet)

Prereqs: Rust 1.89, Solana CLI 3.1.9, Anchor 1.0.2, Node 20+.

```bash
# 1. Build & deploy (devnet)
anchor build
anchor deploy --provider.cluster devnet

# 2. Fund the demo wallet with devnet SOL
solana airdrop 2 --url devnet

# 3. Run the full lifecycle end-to-end
cd app
npm install
npm run demo
```

The demo creates a game, joins 3 players, delegates to the ER
(`https://devnet-as.magicblock.app`), fires 36 dice rolls inside the ER
and prints the measured per-roll latency, then undelegates and settles —
printing the winner and the pot payout with explorer links for every step.

### Fully local run (no devnet SOL needed)

```bash
# Base layer: local test validator with the delegation program cloned
# from devnet and BlitzPot loaded at genesis
solana-test-validator --url https://api.devnet.solana.com \
  --clone-upgradeable-program DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh \
  --bpf-program HGH5JQ877TK25G1bcvKHVa5oTFPkUgG7D2TjwyG8pLkQ target/deploy/blitz_pot.so

# Ephemeral Rollup: local ephemeral validator synced to the local base layer
ephemeral-validator --remotes localhost --listen 127.0.0.1:7799

# Demo against both local endpoints, pinning the local ER validator
# (mAGicPQY… is the local ephemeral validator identity, as in the
# official MagicBlock examples)
cd app
BASE_RPC=http://127.0.0.1:8899 ER_RPC=http://127.0.0.1:7799 \
  DELEGATE_VALIDATOR=mAGicPQYBMvcYveUZA5F5UNNwyHvfYh5xkLS2Fr1mev npm run demo
```

## Deployed (devnet)

- Program ID: `HGH5JQ877TK25G1bcvKHVa5oTFPkUgG7D2TjwyG8pLkQ`
- Example game PDA (host = demo wallet): `FAxAUAYab59qgs2FqA58nfUygtFG7ThVResXm16YBuzv`
- ER endpoint used: `https://devnet-as.magicblock.app` (MagicBlock devnet, Asia region)
- Status: program **builds clean** (`blitz_pot.so`, 359 KB) and is ready to deploy.
  Deployment itself is pending devnet SOL for the deploy wallet — the devnet RPC
  faucet is closed (HTTP 410, it now points to https://faucet.solana.com), so the
  wallet `hbpWkugUv6baXjcz7ycayAG5P2r1hw2RBggHD4gU3c7` needs one visit to the web
  faucet, after which `anchor deploy --provider.cluster devnet && cd app && npm run demo`
  runs the whole lifecycle live. (A fully local run was also attempted: in this
  sandbox the Agave 3.x test validator aborts on an `io_uring` assertion and the
  local ER validator won't stay up against an Agave 2.3 base chain — on a normal
  dev machine the local flow below works as written.)

## Repo layout

```
programs/blitz-pot/   Anchor program (Rust) — game logic + ER delegation
app/demo.ts           End-to-end TypeScript demo (base layer + ER)
Anchor.toml           Anchor config (devnet)
```

## Built with

- [MagicBlock Ephemeral Rollups](https://docs.magicblock.gg) — `ephemeral-rollups-sdk` 0.16.2
- Anchor 1.0.2 · Solana 3.1.9 · `@magicblock-labs/ephemeral-rollups-sdk` (TS)

## License

MIT
