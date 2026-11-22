use anchor_lang::prelude::*;
use anchor_lang::system_program::{self, Transfer};
use ephemeral_rollups_sdk::anchor::{commit, delegate, ephemeral};
use ephemeral_rollups_sdk::cpi::DelegateConfig;
use ephemeral_rollups_sdk::ephem::MagicIntentBundleBuilder;

declare_id!("HGH5JQ877TK25G1bcvKHVa5oTFPkUgG7D2TjwyG8pLkQ");

pub const GAME_SEED: &[u8] = b"game";
pub const MAX_PLAYERS: usize = 8;

/// Game lifecycle:
/// 0 = Lobby    (base layer: players join and pay the entry fee)
/// 1 = Live     (state PDA delegated to an Ephemeral Rollup; dice rolls run there in real time)
/// 2 = Settled  (back on the base layer; pot paid to the winner)
pub const STATE_LOBBY: u8 = 0;
pub const STATE_LIVE: u8 = 1;
pub const STATE_SETTLED: u8 = 2;

#[ephemeral]
#[program]
pub mod blitz_pot {
    use super::*;

    /// Create a new game lobby. The game PDA also escrows the pot:
    /// entry fees are transferred straight into it and the winner is
    /// paid out of it at settle time.
    pub fn create_game(ctx: Context<CreateGame>, entry_fee: u64, max_players: u8) -> Result<()> {
        require!(
            max_players >= 2 && (max_players as usize) <= MAX_PLAYERS,
            BlitzError::InvalidMaxPlayers
        );
        require!(entry_fee > 0, BlitzError::InvalidEntryFee);
        let game = &mut ctx.accounts.game;
        game.host = ctx.accounts.host.key();
        game.entry_fee = entry_fee;
        game.max_players = max_players;
        game.player_count = 0;
        game.players = [Pubkey::default(); MAX_PLAYERS];
        game.scores = [0; MAX_PLAYERS];
        game.rolls = [0; MAX_PLAYERS];
        game.state = STATE_LOBBY;
        game.winner = Pubkey::default();
        game.bump = ctx.bumps.game;
        msg!(
            "BlitzPot game created by {} — entry fee {} lamports, up to {} players",
            game.host,
            entry_fee,
            max_players
        );
        Ok(())
    }

    /// Join a lobby on the base layer, paying the entry fee into the pot.
    pub fn join_game(ctx: Context<JoinGame>) -> Result<()> {
        let game = &mut ctx.accounts.game;
        require!(game.state == STATE_LOBBY, BlitzError::WrongState);
        require!(
            (game.player_count as usize) < (game.max_players as usize),
            BlitzError::GameFull
        );
        let player = ctx.accounts.player.key();
        require!(
            !game.players[..game.player_count as usize].contains(&player),
            BlitzError::AlreadyJoined
        );
        // Pay the entry fee into the game PDA (the pot).
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                Transfer {
                    from: ctx.accounts.player.to_account_info(),
                    to: game.to_account_info(),
                },
            ),
            game.entry_fee,
        )?;
        let idx = game.player_count as usize;
        game.players[idx] = player;
        game.player_count += 1;
        msg!(
            "Player {} joined (slot {}), pot is now {} lamports",
            player,
            idx,
            game.entry_fee * game.player_count as u64
        );
        Ok(())
    }

    /// Lock the lobby and mark the game live. Sent in the same transaction
    /// as `delegate` by the client, right before the game state moves
    /// into the Ephemeral Rollup.
    pub fn start_game(ctx: Context<StartGame>) -> Result<()> {
        let game = &mut ctx.accounts.game;
        require!(game.state == STATE_LOBBY, BlitzError::WrongState);
        require!(game.player_count >= 2, BlitzError::NotEnoughPlayers);
        game.state = STATE_LIVE;
        msg!(
            "Game is LIVE with {} players — moving to the Ephemeral Rollup",
            game.player_count
        );
        Ok(())
    }

    /// Delegate the game PDA to the delegation program so the dice phase
    /// can execute inside an Ephemeral Rollup (same pattern as the
    /// official MagicBlock Anchor counter example).
    pub fn delegate(ctx: Context<DelegateInput>) -> Result<()> {
        ctx.accounts.delegate_pda(
            &ctx.accounts.payer,
            &[GAME_SEED, ctx.accounts.host.key().as_ref()],
            DelegateConfig {
                // Optionally pin a specific validator from the first remaining account
                validator: ctx.remaining_accounts.first().map(|acc| acc.key()),
                ..Default::default()
            },
        )?;
        Ok(())
    }

    /// Roll the dice. Executed inside the Ephemeral Rollup: near-zero
    /// latency and zero fees, so players can roll many times per game.
    ///
    /// Randomness note (MVP): the roll is derived from the player key,
    /// their roll nonce and the ER clock (slot + timestamp). That is fine
    /// for a devnet demo but is *not* secure randomness — a production
    /// version should use MagicBlock VRF (see README).
    pub fn roll(ctx: Context<Roll>) -> Result<()> {
        let game = &mut ctx.accounts.game;
        require!(game.state == STATE_LIVE, BlitzError::WrongState);
        let player = ctx.accounts.player.key();
        let idx = game.players[..game.player_count as usize]
            .iter()
            .position(|p| *p == player)
            .ok_or(BlitzError::NotAPlayer)?;
        let clock = Clock::get()?;
        let nonce = game.rolls[idx];
        // xorshift64* mix of player key, roll nonce and the ER clock.
        let player_bytes = player.to_bytes();
        let mut x = clock.slot
            ^ (clock.unix_timestamp as u64).rotate_left(17)
            ^ nonce.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ u64::from_le_bytes(player_bytes[0..8].try_into().unwrap());
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let value = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) % 6) + 1;
        game.scores[idx] += value;
        game.rolls[idx] += 1;
        msg!(
            "Player {} rolled a {} (score {}, rolls {})",
            player,
            value,
            game.scores[idx],
            game.rolls[idx]
        );
        Ok(())
    }

    /// Commit the current ER state back to the base layer, staying delegated.
    pub fn commit(ctx: Context<CommitGame>) -> Result<()> {
        MagicIntentBundleBuilder::new(
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.magic_context.to_account_info(),
            ctx.accounts.magic_program.to_account_info(),
        )
        .commit(&[ctx.accounts.game.to_account_info()])
        .build_and_invoke()?;
        Ok(())
    }

    /// Final commit + undelegate: the finished game state lands back on
    /// the base layer, owned by this program again, ready to settle.
    pub fn undelegate(ctx: Context<CommitGame>) -> Result<()> {
        // Persist the in-memory state before handing it to the magic program
        ctx.accounts.game.exit(&crate::ID)?;
        MagicIntentBundleBuilder::new(
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.magic_context.to_account_info(),
            ctx.accounts.magic_program.to_account_info(),
        )
        .commit_and_undelegate(&[ctx.accounts.game.to_account_info()])
        .build_and_invoke()?;
        Ok(())
    }

    /// Settle on the base layer after undelegation: the highest score
    /// wins the whole pot. Permissionless — the payout is deterministic.
    /// Ties go to the earliest joiner.
    pub fn settle(ctx: Context<SettleGame>) -> Result<()> {
        let game = &mut ctx.accounts.game;
        require!(game.state == STATE_LIVE, BlitzError::WrongState);
        let mut winner_idx = 0usize;
        for i in 1..game.player_count as usize {
            if game.scores[i] > game.scores[winner_idx] {
                winner_idx = i;
            }
        }
        let winner = game.players[winner_idx];
        require!(
            ctx.accounts.winner.key() == winner,
            BlitzError::WinnerMismatch
        );
        let pot = game.entry_fee * game.player_count as u64;
        game.state = STATE_SETTLED;
        game.winner = winner;
        // Pay the pot out of the game PDA (owned by this program).
        let game_info = game.to_account_info();
        **game_info.try_borrow_mut_lamports()? -= pot;
        let winner_info = ctx.accounts.winner.to_account_info();
        **winner_info.try_borrow_mut_lamports()? += pot;
        msg!(
            "Game settled — winner {} takes the pot of {} lamports (score {})",
            winner,
            pot,
            game.scores[winner_idx]
        );
        Ok(())
    }
}

#[derive(Accounts)]
pub struct CreateGame<'info> {
    #[account(
        init,
        payer = host,
        space = Game::SPACE,
        seeds = [GAME_SEED, host.key().as_ref()],
        bump
    )]
    pub game: Account<'info, Game>,
    #[account(mut)]
    pub host: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct JoinGame<'info> {
    #[account(mut, seeds = [GAME_SEED, host.key().as_ref()], bump = game.bump)]
    pub game: Account<'info, Game>,
    /// CHECK: host pubkey is only used to derive the game PDA
    pub host: UncheckedAccount<'info>,
    #[account(mut)]
    pub player: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct StartGame<'info> {
    #[account(mut, seeds = [GAME_SEED, host.key().as_ref()], bump = game.bump)]
    pub game: Account<'info, Game>,
    pub host: Signer<'info>,
}

/// Delegation context — shape injected by the `#[delegate]` macro
/// (buffer / delegation record / metadata / delegation program / system program).
#[delegate]
#[derive(Accounts)]
pub struct DelegateInput<'info> {
    pub payer: Signer<'info>,
    /// CHECK: host pubkey is only used to derive the game PDA seeds
    pub host: UncheckedAccount<'info>,
    /// CHECK: the game PDA to delegate
    #[account(mut, del)]
    pub pda: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Roll<'info> {
    #[account(mut, seeds = [GAME_SEED, host.key().as_ref()], bump)]
    pub game: Account<'info, Game>,
    /// CHECK: host pubkey is only used to derive the game PDA
    pub host: UncheckedAccount<'info>,
    pub player: Signer<'info>,
}

/// Commit / undelegate context — `#[commit]` injects the magic
/// context and magic program accounts.
#[commit]
#[derive(Accounts)]
pub struct CommitGame<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut, seeds = [GAME_SEED, host.key().as_ref()], bump)]
    pub game: Account<'info, Game>,
    /// CHECK: host pubkey is only used to derive the game PDA
    pub host: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct SettleGame<'info> {
    #[account(mut, seeds = [GAME_SEED, host.key().as_ref()], bump = game.bump)]
    pub game: Account<'info, Game>,
    /// CHECK: host pubkey is only used to derive the game PDA
    pub host: UncheckedAccount<'info>,
    /// CHECK: the winner is read from game state; lamports are credited here.
    /// Must match game.winner-to-be (highest scorer) — enforced in the handler
    /// by comparing against the computed winner.
    #[account(mut)]
    pub winner: UncheckedAccount<'info>,
}

#[account]
pub struct Game {
    pub host: Pubkey,
    pub entry_fee: u64,
    pub max_players: u8,
    pub player_count: u8,
    pub players: [Pubkey; MAX_PLAYERS],
    pub scores: [u64; MAX_PLAYERS],
    pub rolls: [u64; MAX_PLAYERS],
    pub state: u8,
    pub winner: Pubkey,
    pub bump: u8,
}

impl Game {
    pub const SPACE: usize = 8      // discriminator
        + 32                        // host
        + 8                         // entry_fee
        + 1                         // max_players
        + 1                         // player_count
        + 32 * MAX_PLAYERS          // players
        + 8 * MAX_PLAYERS           // scores
        + 8 * MAX_PLAYERS           // rolls
        + 1                         // state
        + 32                        // winner
        + 1; // bump
}

#[error_code]
pub enum BlitzError {
    #[msg("max_players must be between 2 and 8")]
    InvalidMaxPlayers,
    #[msg("entry fee must be greater than zero")]
    InvalidEntryFee,
    #[msg("game is not in the expected state for this action")]
    WrongState,
    #[msg("game is full")]
    GameFull,
    #[msg("player has already joined this game")]
    AlreadyJoined,
    #[msg("at least 2 players are required to start")]
    NotEnoughPlayers,
    #[msg("signer is not a player in this game")]
    NotAPlayer,
    #[msg("winner account does not match the computed winner")]
    WinnerMismatch,
}
