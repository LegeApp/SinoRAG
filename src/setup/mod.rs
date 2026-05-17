//! `sinorag setup <agent>` — provider-specific onboarding checks.
//!
//! Each agent SinoRAG can wrap (currently just opencode) gets its own
//! subcommand under `setup`. The check is intentionally read-only: we
//! verify the agent is reachable, print install instructions if it isn't,
//! and remind the user to configure a model provider. Nothing is installed
//! automatically — package managers vary per platform and we'd rather be
//! transparent about what to run.

pub mod opencode;
