/**
 * BlitzPot — end-to-end demo on Solana devnet + MagicBlock Ephemeral Rollup.
 *
 * Lifecycle demonstrated:
 *   1. Host creates a game lobby on the base layer (devnet).
 *   2. Three players join, each paying the entry fee into the pot.
 *   3. Host starts the game and delegates the game PDA to an Ephemeral Rollup.
 *   4. Players roll dice *inside the ER* — fast, zero-fee, real-time.
 *   5. State is committed and undelegated back to the base layer.
 *   6. Anyone calls settle: highest score wins the whole pot.
 *
 * Usage:
 *   1. `anchor build` in the repo root (produces target/idl/blitz_pot.json)
 *   2. `anchor deploy --provider.cluster devnet`
 *   3. `cd app && npm install && npm run demo`
 *
 * The demo wallet is read from ~/.config/solana/id.json and must hold a
 * little devnet SOL (run `solana airdrop 2` first). All other players are
 * fresh keypairs funded by the host inside the script. Devnet only —
 * no real funds are involved anywhere.
 */
import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import {
  Connection,
  Keypair,
  LAMPORTS_PER_SOL,
  PublicKey,
  SystemProgram,
  Transaction,
} from "@solana/web3.js";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

const BASE_RPC = process.env.BASE_RPC ?? "https://api.devnet.solana.com";
// A specific ER region endpoint (the router endpoint https://devnet.magicblock.app also works)
const ER_RPC = process.env.ER_RPC ?? "https://devnet-as.magicblock.app";
const ENTRY_FEE = 0.05 * LAMPORTS_PER_SOL; // 0.05 devnet SOL per player
const MAX_PLAYERS = 4;
const ROLLS_PER_PLAYER = 12;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function loadWallet(): Keypair {
  const p = path.join(os.homedir(), ".config", "solana", "id.json");
  const secret = JSON.parse(fs.readFileSync(p, "utf8"));
  return Keypair.fromSecretKey(Uint8Array.from(secret));
}

async function airdropIfBroke(conn: Connection, who: PublicKey, label: string) {
  const bal = await conn.getBalance(who);
  if (bal < 0.2 * LAMPORTS_PER_SOL) {
    console.log(`  airdropping 1 devnet SOL to ${label}…`);
    const sig = await conn.requestAirdrop(who, 1 * LAMPORTS_PER_SOL);
    await conn.confirmTransaction(sig, "confirmed");
  }
}

async function main() {
  const idlPath = path.join(__dirname, "..", "target", "idl", "blitz_pot.json");
  if (!fs.existsSync(idlPath)) {
    throw new Error(`IDL not found at ${idlPath} — run \`anchor build\` first`);
  }
  const idl = JSON.parse(fs.readFileSync(idlPath, "utf8"));

  const host = loadWallet();
  const baseConn = new Connection(BASE_RPC, "confirmed");
  const erConn = new Connection(ER_RPC, "confirmed");

  const baseProvider = new anchor.AnchorProvider(
    baseConn,
    new anchor.Wallet(host),
    { commitment: "confirmed" }
  );
  const program = new Program(idl, baseProvider);
  const programId = program.programId;
  console.log(`BlitzPot program: ${programId.toBase58()}`);
  console.log(`Host: ${host.publicKey.toBase58()}`);

  await airdropIfBroke(baseConn, host.publicKey, "host");

  // Game PDA
  const [gamePda] = PublicKey.findProgramAddressSync(
    [Buffer.from("game"), host.publicKey.toBuffer()],
    programId
  );
  console.log(`Game PDA: ${gamePda.toBase58()}`);

  // Two guest players, funded by the host
  const guests = [Keypair.generate(), Keypair.generate()];
  for (const [i, g] of guests.entries()) {
    const bal = await baseConn.getBalance(g.publicKey);
    if (bal < 0.15 * LAMPORTS_PER_SOL) {
      const tx = new Transaction().add(
        SystemProgram.transfer({
          fromPubkey: host.publicKey,
          toPubkey: g.publicKey,
          lamports: 0.3 * LAMPORTS_PER_SOL,
        })
      );
      await baseProvider.sendAndConfirm(tx, []);
      console.log(`  funded guest ${i + 1}: ${g.publicKey.toBase58()}`);
    }
  }
  const players: Keypair[] = [host, ...guests];
  const providerFor = (kp: Keypair, er = false) =>
    new anchor.AnchorProvider(er ? erConn : baseConn, new anchor.Wallet(kp), {
      commitment: er ? "processed" : "confirmed",
      skipPreflight: er,
    });
  const programFor = (kp: Keypair, er = false) =>
    new Program(idl, providerFor(kp, er)) as any;

  // ── 1. Create the game (skip if a previous demo run left one behind) ──
  const existing = await baseConn.getAccountInfo(gamePda);
  if (!existing) {
    const sig = await programFor(host)
      .methods.createGame(new anchor.BN(ENTRY_FEE), MAX_PLAYERS)
      .accounts({ host: host.publicKey })
      .rpc();
    console.log(`1) game created: https://explorer.solana.com/tx/${sig}?cluster=devnet`);
  } else {
    console.log("1) game already exists from a previous run — reusing it");
  }

  // ── 2. Players join on the base layer, paying into the pot ──
  let game: any = await (program as any).account.game.fetch(gamePda);
  for (const p of players) {
    const joined = game.players
      .slice(0, game.playerCount)
      .some((x: PublicKey) => x.equals(p.publicKey));
    if (!joined && game.state === 0) {
      const sig = await programFor(p)
        .methods.joinGame()
        .accounts({ host: host.publicKey, player: p.publicKey })
        .rpc();
      console.log(
        `2) ${p.publicKey.toBase58().slice(0, 8)}… joined: https://explorer.solana.com/tx/${sig}?cluster=devnet`
      );
    }
  }
  game = await (program as any).account.game.fetch(gamePda);
  console.log(
    `   pot = ${(game.entryFee.toNumber() * game.playerCount) / LAMPORTS_PER_SOL} SOL, players = ${game.playerCount}`
  );

  // ── 3. Start + delegate in a single transaction ──
  if (game.state === 0) {
    const startIx = await programFor(host)
      .methods.startGame()
      .accounts({ host: host.publicKey })
      .instruction();
    const delegateIx = await programFor(host)
      .methods.delegate()
      .accounts({ payer: host.publicKey, host: host.publicKey, pda: gamePda })
      .instruction();
    const tx = new Transaction().add(startIx, delegateIx);
    const sig = await baseProvider.sendAndConfirm(tx, []);
    console.log(`3) started + delegated: https://explorer.solana.com/tx/${sig}?cluster=devnet`);
  }

  // Wait until the game account shows up inside the ER
  console.log("   waiting for the game account inside the Ephemeral Rollup…");
  for (let i = 0; i < 30; i++) {
    const info = await erConn.getAccountInfo(gamePda);
    if (info) break;
    if (i === 29) throw new Error("game account never appeared in the ER");
    await sleep(1000);
  }
  console.log("   game is live in the ER ⚡");

  // ── 4. Dice phase inside the ER — rapid-fire rolls, timed ──
  console.log(`4) rolling ${ROLLS_PER_PLAYER} dice per player inside the ER…`);
  const t0 = Date.now();
  let rollCount = 0;
  for (let round = 0; round < ROLLS_PER_PLAYER; round++) {
    await Promise.all(
      players.map(async (p) => {
        await programFor(p, true)
          .methods.roll()
          .accounts({ host: host.publicKey, player: p.publicKey, game: gamePda })
          .rpc();
        rollCount++;
      })
    );
  }
  const elapsed = Date.now() - t0;
  console.log(
    `   ${rollCount} rolls confirmed in ${elapsed} ms (~${Math.round(elapsed / rollCount)} ms per roll)`
  );

  const erGame: any = await programFor(host, true).account.game.fetch(gamePda);
  erGame.players.slice(0, erGame.playerCount).forEach((pk: PublicKey, i: number) => {
    console.log(
      `   player ${pk.toBase58().slice(0, 8)}… — score ${erGame.scores[i].toNumber()}, rolls ${erGame.rolls[i].toNumber()}`
    );
  });

  // ── 5. Commit + undelegate back to the base layer ──
  const undelSig = await programFor(host, true)
    .methods.undelegate()
    .accounts({ payer: host.publicKey, host: host.publicKey, game: gamePda })
    .rpc();
  console.log(`5) committed + undelegated: ${undelSig}`);
  // Give the base layer a moment to receive the committed state
  for (let i = 0; i < 30; i++) {
    const info = await baseConn.getAccountInfo(gamePda);
    if (info && info.owner.equals(programId)) break;
    await sleep(1000);
  }

  // ── 6. Settle on the base layer: winner takes the pot ──
  const finalGame: any = await (program as any).account.game.fetch(gamePda);
  let winnerIdx = 0;
  for (let i = 1; i < finalGame.playerCount; i++) {
    if (finalGame.scores[i].toNumber() > finalGame.scores[winnerIdx].toNumber()) winnerIdx = i;
  }
  const winner = finalGame.players[winnerIdx] as PublicKey;
  const winnerBalBefore = await baseConn.getBalance(winner);
  const sig = await programFor(host)
    .methods.settle()
    .accounts({ host: host.publicKey, winner })
    .rpc();
  const winnerBalAfter = await baseConn.getBalance(winner);
  console.log(`6) settled: https://explorer.solana.com/tx/${sig}?cluster=devnet`);
  console.log(
    `🏆 winner ${winner.toBase58()} received ${(winnerBalAfter - winnerBalBefore) / LAMPORTS_PER_SOL} SOL — pot paid out.`
  );
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
