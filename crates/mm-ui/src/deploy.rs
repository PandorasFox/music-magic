//! Shared deploy staging logic.
//!
//! Pure function: takes DeployModalData + PathResolver, produces decision
//! tuples. Both mm-tui and mm-web call this identically. Client-specific
//! code handles the actual transaction staging.

use mm_meta::decisions::DecisionKey;
use mm_meta::mutations::Mutation;
use mm_meta::paths::PathResolver;
use mm_meta::views::cluster_deploy::DeployModalData;
use mm_meta::views::DeploySignalFile;

use crate::decision_keys;

/// A decision ready to be staged into a transaction.
pub struct DeployDecision {
    pub key: DecisionKey,
    pub label: String,
    pub mutations: Vec<Mutation>,
}

/// Result of preparing deploy decisions.
pub struct PreparedDeploy {
    pub decisions: Vec<DeployDecision>,
    /// Files skipped due to missing library_name (config gap).
    pub skipped: usize,
}

/// Convert deploy modal data into decisions ready for staging.
///
/// Both TUI and web call this identically; client code then stages the
/// returned decisions using platform-specific mechanisms (gesture exchange
/// in TUI, transaction API in web).
///
/// `new_files` is the per-file deploy-ready list, fetched separately via
/// `GetDeployReadyFiles` — the modal payload carries only counts.
pub fn prepare_deploy_decisions(
    data: &DeployModalData,
    new_files: &[DeploySignalFile],
    resolver: &PathResolver,
) -> PreparedDeploy {
    let mutation_set = data.to_mutations(new_files, resolver);

    let mut decisions = Vec::new();

    if !mutation_set.deploy.is_empty() {
        decisions.push(DeployDecision {
            key: decision_keys::deploy(),
            label: "Deploy operations".to_string(),
            mutations: mutation_set.deploy,
        });
    }

    if !mutation_set.sidecars.is_empty() {
        decisions.push(DeployDecision {
            key: decision_keys::deploy_sidecars(),
            label: format!("Deploy cover art ({} images)", mutation_set.sidecars.len()),
            mutations: mutation_set.sidecars,
        });
    }

    PreparedDeploy {
        decisions,
        skipped: mutation_set.skipped,
    }
}
