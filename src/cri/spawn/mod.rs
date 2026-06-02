//! Container spawn pipeline (Wave 7 C1).

pub mod child;
mod context;
mod pipes;
mod pipeline;
mod post_fork;
mod steps;

pub use context::{ContainerSpawnCtx, IsolationStrategy, isolation_strategy};
pub use pipes::{StdPipes, create_std_pipes};
pub use pipeline::{SpawnPipeline, SpawnState, SpawnStep};
