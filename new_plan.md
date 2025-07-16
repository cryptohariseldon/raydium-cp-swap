# Raydium CP-Swap MEV Protection Implementation Plan

## Overview
This document outlines the comprehensive plan to implement MEV protection for Raydium CP-Swap through custom authority pools and FIFO ordering via the Continuum wrapper.

## Key Advantages of CP-Swap
- **No OpenBook Dependency**: CP-Swap is a concentrated liquidity AMM that doesn't require OpenBook/Serum markets
- **Simpler Architecture**: Direct token-to-token swaps without orderbook complexity
- **Modern Design**: Uses Anchor framework with cleaner code structure
- **Better Gas Efficiency**: Lower compute units per swap

## 1. Raydium CP-Swap Modifications

### 1.1 Pool State Modifications
**File**: `raydium-cp-swap/programs/cp-swap/src/states/pool.rs`

```rust
// Current PoolState struct modifications needed:
pub struct PoolState {
    // ... existing fields ...
    
    /// Authority type: 0 = Default PDA, 1 = Custom
    pub authority_type: u8,
    /// Custom authority (used when authority_type == 1)
    pub custom_authority: Pubkey,
    /// Pool authority bump (for custom authority validation)
    pub custom_auth_bump: u8,
    
    // Reduce padding to accommodate new fields
    pub padding: [u64; 29], // reduced from 31
}
```

### 1.2 Initialize Instruction Modifications
**File**: `raydium-cp-swap/programs/cp-swap/src/instructions/initialize.rs`

Key changes:
1. Add parameters for custom authority configuration
2. Modify pool initialization to set custom authority
3. Update authority validation logic

```rust
pub fn initialize(
    ctx: Context<Initialize>,
    init_amount_0: u64,
    init_amount_1: u64,
    open_time: u64,
    authority_type: u8,           // NEW
    custom_authority: Option<Pubkey>, // NEW
) -> Result<()> {
    // ... existing logic ...
    
    // Set authority configuration
    pool_state.authority_type = authority_type;
    pool_state.custom_authority = custom_authority.unwrap_or_default();
}
```

### 1.3 Authority Validation Helper
**New File**: `raydium-cp-swap/programs/cp-swap/src/utils/authority.rs`

```rust
pub fn get_pool_authority(pool_state: &PoolState) -> Pubkey {
    if pool_state.authority_type == 1 {
        pool_state.custom_authority
    } else {
        // Default PDA derivation
        Pubkey::find_program_address(
            &[AUTH_SEED.as_bytes()],
            &crate::ID
        ).0
    }
}
```

### 1.4 Swap Instruction Modifications
**Files**: 
- `raydium-cp-swap/programs/cp-swap/src/instructions/swap_base_input.rs`
- `raydium-cp-swap/programs/cp-swap/src/instructions/swap_base_output.rs`

Add authority validation for custom pools:
```rust
// In swap functions, validate authority
if pool_state.authority_type == 1 {
    // Ensure swap is authorized by custom authority
    require!(
        ctx.accounts.authority.key() == pool_state.custom_authority,
        ErrorCode::InvalidAuthority
    );
}
```

## 2. Continuum Wrapper Enhancements

### 2.1 Core Architecture Updates
**File**: `continuum-wrapper/src/lib.rs`

Key components:
1. **FIFO State**: Global sequence counter for order management
2. **Pool Registry**: Track CP-Swap pools under Continuum control
3. **Order Queue**: Store pending orders with metadata

```rust
#[account]
pub struct FifoState {
    pub current_sequence: u64,
    pub admin: Pubkey,
    pub emergency_pause: bool,
}

#[account]
pub struct CpSwapPoolRegistry {
    pub pool_id: Pubkey,
    pub token_0: Pubkey,
    pub token_1: Pubkey,
    pub continuum_authority: Pubkey,
    pub created_at: i64,
}

#[account]
pub struct OrderState {
    pub sequence: u64,
    pub user: Pubkey,
    pub pool_id: Pubkey,
    pub amount_in: u64,
    pub min_amount_out: u64,
    pub is_token_0_to_1: bool,
    pub status: OrderStatus,
    pub submitted_at: i64,
}
```

### 2.2 Instructions

#### 2.2.1 Initialize CP-Swap Pool
```rust
pub fn initialize_cp_swap_pool(
    ctx: Context<InitializeCpSwapPool>,
    init_amount_0: u64,
    init_amount_1: u64,
    open_time: u64,
) -> Result<()> {
    // 1. Calculate Continuum pool authority PDA
    let (pool_authority, bump) = Pubkey::find_program_address(
        &[b"cp_pool_authority", pool_id.as_ref()],
        &ctx.program_id
    );
    
    // 2. CPI to CP-Swap with custom authority
    let cpi_accounts = Initialize {
        // ... all required accounts ...
    };
    
    let cpi_ctx = CpiContext::new(
        ctx.accounts.cp_swap_program.to_account_info(),
        cpi_accounts
    );
    
    cp_swap::initialize(
        cpi_ctx,
        init_amount_0,
        init_amount_1,
        open_time,
        1, // authority_type = Custom
        Some(pool_authority),
    )?;
    
    // 3. Register pool in Continuum
    let registry = &mut ctx.accounts.pool_registry;
    registry.pool_id = pool_id;
    registry.continuum_authority = pool_authority;
    
    Ok(())
}
```

#### 2.2.2 Submit Order
```rust
pub fn submit_order(
    ctx: Context<SubmitOrder>,
    amount_in: u64,
    min_amount_out: u64,
    is_token_0_to_1: bool,
) -> Result<()> {
    let fifo_state = &mut ctx.accounts.fifo_state;
    let order_state = &mut ctx.accounts.order_state;
    
    // Assign sequence number
    let sequence = fifo_state.current_sequence + 1;
    fifo_state.current_sequence = sequence;
    
    // Store order details
    order_state.sequence = sequence;
    order_state.user = ctx.accounts.user.key();
    order_state.pool_id = ctx.accounts.pool_id;
    order_state.amount_in = amount_in;
    order_state.min_amount_out = min_amount_out;
    order_state.is_token_0_to_1 = is_token_0_to_1;
    order_state.status = OrderStatus::Pending;
    order_state.submitted_at = Clock::get()?.unix_timestamp;
    
    emit!(OrderSubmitted {
        sequence,
        user: ctx.accounts.user.key(),
        pool_id: ctx.accounts.pool_id,
        amount_in,
    });
    
    Ok(())
}
```

#### 2.2.3 Execute Order
```rust
pub fn execute_order(
    ctx: Context<ExecuteOrder>,
    expected_sequence: u64,
) -> Result<()> {
    let order = &ctx.accounts.order_state;
    
    // Validate FIFO sequence
    require!(
        order.sequence == expected_sequence,
        ErrorCode::InvalidSequence
    );
    require!(
        order.status == OrderStatus::Pending,
        ErrorCode::OrderAlreadyProcessed
    );
    
    // Transfer tokens from user to Continuum
    // (User pre-approves Continuum as delegate)
    
    // Build swap CPI
    let swap_accounts = if order.is_token_0_to_1 {
        SwapBaseInput {
            // ... accounts for token0 -> token1 swap ...
        }
    } else {
        SwapBaseInput {
            // ... accounts for token1 -> token0 swap ...
        }
    };
    
    // Execute swap with pool authority
    let seeds = &[
        b"cp_pool_authority",
        order.pool_id.as_ref(),
        &[ctx.bumps.pool_authority],
    ];
    
    let signer_seeds = &[&seeds[..]];
    
    cp_swap::swap_base_input(
        CpiContext::new_with_signer(
            ctx.accounts.cp_swap_program.to_account_info(),
            swap_accounts,
            signer_seeds,
        ),
        order.amount_in,
        order.min_amount_out,
    )?;
    
    // Update order status
    order.status = OrderStatus::Executed;
    
    emit!(OrderExecuted {
        sequence: order.sequence,
        user: order.user,
    });
    
    Ok(())
}
```

## 3. Client SDK Implementation

### 3.1 Core SDK Structure
```
sdk/
├── src/
│   ├── index.ts
│   ├── types/
│   │   ├── orders.ts
│   │   └── pools.ts
│   ├── instructions/
│   │   ├── initialize-pool.ts
│   │   ├── submit-order.ts
│   │   └── query-orders.ts
│   ├── utils/
│   │   ├── pda.ts
│   │   └── tokens.ts
│   └── client/
│       ├── continuum-client.ts
│       └── websocket-client.ts
└── tests/
```

### 3.2 Key SDK Functions

#### 3.2.1 Pool Creation
```typescript
export async function createCpSwapPool(
  connection: Connection,
  payer: Keypair,
  token0: PublicKey,
  token1: PublicKey,
  initialLiquidity: {
    amount0: BN,
    amount1: BN,
  }
): Promise<{
  poolId: PublicKey,
  continuumAuthority: PublicKey,
  signature: string,
}> {
  // Implementation details
}
```

#### 3.2.2 Order Submission
```typescript
export async function submitSwapOrder(
  connection: Connection,
  user: Keypair,
  poolId: PublicKey,
  params: {
    amountIn: BN,
    minAmountOut: BN,
    inputToken: PublicKey,
    outputToken: PublicKey,
  }
): Promise<{
  orderId: PublicKey,
  sequence: BN,
  signature: string,
}> {
  // Implementation details
}
```

#### 3.2.3 Order Monitoring
```typescript
export class OrderMonitor {
  constructor(
    private connection: Connection,
    private poolId: PublicKey
  ) {}
  
  async watchOrders(callback: (order: Order) => void) {
    // WebSocket subscription for order events
  }
  
  async getPendingOrders(): Promise<Order[]> {
    // Query pending orders
  }
}
```

## 4. Relayer Implementation

### 4.1 Architecture
```
relayer/
├── src/
│   ├── main.rs
│   ├── executor/
│   │   ├── mod.rs
│   │   ├── order_processor.rs
│   │   └── transaction_builder.rs
│   ├── monitor/
│   │   ├── mod.rs
│   │   ├── order_watcher.rs
│   │   └── pool_scanner.rs
│   ├── config.rs
│   └── metrics.rs
└── Cargo.toml
```

### 4.2 Core Components

#### 4.2.1 Order Processor
```rust
pub struct OrderProcessor {
    rpc_client: RpcClient,
    continuum_program: Pubkey,
    cp_swap_program: Pubkey,
    executor_keypair: Keypair,
}

impl OrderProcessor {
    pub async fn process_next_order(&self, pool_id: Pubkey) -> Result<()> {
        // 1. Get next expected sequence
        let fifo_state = self.get_fifo_state().await?;
        let next_sequence = fifo_state.current_sequence + 1;
        
        // 2. Find order with matching sequence
        let order = self.find_order_by_sequence(pool_id, next_sequence).await?;
        
        // 3. Build and send transaction
        let tx = self.build_execute_transaction(order).await?;
        let signature = self.rpc_client.send_and_confirm_transaction(&tx).await?;
        
        info!("Executed order {} with signature {}", next_sequence, signature);
        Ok(())
    }
}
```

#### 4.2.2 Pool Monitor
```rust
pub struct PoolMonitor {
    pools: Vec<PoolInfo>,
    websocket_client: PubsubClient,
}

impl PoolMonitor {
    pub async fn start_monitoring(&mut self) -> Result<()> {
        // Subscribe to order submission events
        let (mut stream, _) = self.websocket_client
            .program_subscribe(&self.continuum_program, None)
            .await?;
            
        while let Some(event) = stream.next().await {
            if let Ok(order_event) = parse_order_event(event) {
                self.handle_new_order(order_event).await?;
            }
        }
        
        Ok(())
    }
}
```

### 4.3 Configuration
```toml
[relayer]
rpc_url = "https://api.devnet.solana.com"
ws_url = "wss://api.devnet.solana.com"
executor_keypair_path = "./keypair.json"

[programs]
continuum = "9Mp8VkLRUR1Gw6HSXmByjM4tqabaDnoTpDpbzMvsiQ2Y"
cp_swap = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C"

[monitoring]
poll_interval_ms = 100
max_concurrent_executions = 10
retry_attempts = 3
```

## 5. Implementation Timeline

### Phase 1: CP-Swap Modifications (Week 1-2)
- [ ] Modify pool state structure
- [ ] Update initialization logic
- [ ] Implement authority validation
- [ ] Add custom authority tests

### Phase 2: Continuum Wrapper Updates (Week 2-3)
- [ ] Design CP-Swap specific instructions
- [ ] Implement pool creation with custom authority
- [ ] Build order submission and execution logic
- [ ] Add comprehensive tests

### Phase 3: SDK Development (Week 3-4)
- [ ] Create TypeScript SDK structure
- [ ] Implement pool creation helpers
- [ ] Build order submission interface
- [ ] Add WebSocket monitoring

### Phase 4: Relayer Implementation (Week 4-5)
- [ ] Set up Rust project structure
- [ ] Implement order monitoring
- [ ] Build execution engine
- [ ] Add metrics and logging

### Phase 5: Integration Testing (Week 5-6)
- [ ] End-to-end testing on devnet
- [ ] Performance optimization
- [ ] Security audit preparation
- [ ] Documentation

## 6. Security Considerations

### 6.1 Authority Control
- Custom authority cannot be changed after pool creation
- Only Continuum can execute swaps on protected pools
- Pool creator cannot bypass FIFO ordering

### 6.2 FIFO Enforcement
- Global sequence counter prevents manipulation
- Orders must be executed in submission order
- No ability to cancel or reorder

### 6.3 Economic Security
- Minimum order size to prevent spam
- Gas fees paid by users
- Relayer incentives for execution

## 7. Advantages Over V4 Implementation

1. **No OpenBook Dependency**: Simpler architecture without market creation
2. **Lower Complexity**: Direct pool operations without orderbook
3. **Better Performance**: Fewer accounts and lower compute usage
4. **Modern Codebase**: Anchor framework with better developer experience
5. **Concentrated Liquidity**: More capital efficient than constant product

## 8. Migration Strategy

For existing V4 pools:
1. Deploy new CP-Swap pools with same token pairs
2. Incentivize liquidity migration
3. Gradually phase out V4 pool usage
4. Maintain both during transition period

## Conclusion

This implementation plan provides a comprehensive approach to adding MEV protection to Raydium CP-Swap. The custom authority mechanism ensures that pools under Continuum control can only be accessed through the FIFO ordering system, preventing sandwich attacks and other MEV exploitation.

The CP-Swap implementation is superior to modifying V4 due to its independence from OpenBook and cleaner architecture. This approach provides a more maintainable and efficient solution for MEV-protected swaps.