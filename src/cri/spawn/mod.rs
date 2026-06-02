//! Container spawn pipeline (Wave 7 C1).

mod context;
mod pipes;
mod pipeline;

pub use context::{ContainerSpawnCtx, IsolationStrategy, isolation_strategy};
pub use pipes::{StdPipes, create_std_pipes};
pub use pipeline::{SpawnPipeline, SpawnState, SpawnStep};
