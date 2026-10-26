//! Schedule Content Analysis executor.


use crate::logging::log_general;
use crate::meta::computations::helpers::get_configured_library_names;
use crate::meta::computations::traits::ComputationContext;
use crate::meta::recomputation::RecomputationScope;

use super::{Computation, Result};

/// Execute ScheduleContentAnalysis - spawns content detection computations
/// filtered by recomputation scope.
///
/// - `scope = None`: startup — spawn all computations.
/// - `scope = Some(s)`: post-mutation — spawn only computations whose domains
///   overlap with `s`.
pub fn execute_schedule_content_analysis(
    ctx: &ComputationContext<'_>,
    scope: &Option<RecomputationScope>,
) -> Result {
    let run_all = scope.is_none();
    let s = scope.unwrap_or(RecomputationScope::EMPTY);

    log_general(format!(
        "[COMPUTE] ScheduleContentAnalysis: scope={:?} (run_all={})",
        scope, run_all
    ));

    // Note: OOB tag change classification is now handled in Observation phase by VerifyTags
    //
    // AnalyzeFingerprintOverlaps and DetectCrossSourceOverlaps are NOT spawned here.
    // They depend on FingerprintOverlap signals written by DetectFingerprintOverlaps,
    // so DetectFingerprintOverlaps defers them behind a pipeline barrier.

    let mut spawn = Vec::new();

    // Tag-sensitive computations
    if run_all || s.contains(RecomputationScope::TAGS) {
        spawn.extend([
            Computation::DetectMusicBrainzTagged,
            Computation::DetectMissingTags,
            Computation::DetectInconsistentAlbumArtist,
            Computation::DetectCompoundTagValues,
            Computation::DetectDiscExtractions,
            Computation::ImportGenresFromTags,
        ]);

        // Album/artist/albumartist collision detection is expensive and
        // gated by opinions.canonicalization.enable-string-canonicalizations
        // (default: off).
        let enable_string_canon = ctx
            .snapshot
            .config
            .as_deref()
            .is_some_and(|c| c.opinions.canonicalization.enable_string_canonicalizations);
        if enable_string_canon {
            spawn.extend([
                Computation::DetectArtistTagCanonicalizations,
                Computation::DetectAlbumArtistTagCanonicalizations,
                Computation::DetectAlbumTagCanonicalizations,
            ]);
        }

        // Genre collision detection is gated by
        // opinions.canonicalization.enable-genre-canonicalizations (default:
        // off) while the genre canonicalization design is being reworked.
        let enable_genre_canon = ctx
            .snapshot
            .config
            .as_deref()
            .is_some_and(|c| c.opinions.canonicalization.enable_genre_canonicalizations);
        if enable_genre_canon {
            spawn.push(Computation::DetectGenreTagCanonicalizations);
        }
    }

    // File/identity-sensitive computations
    if run_all || s.contains(RecomputationScope::FILES) {
        spawn.extend([
            Computation::DetectDuplicateInodes,
            Computation::DetectLosslessRemux,
        ]);

        // DetectFingerprintOverlaps (+ its downstream AnalyzeFingerprintOverlaps
        // and DetectCrossSourceOverlaps) is expensive and gated behind
        // opinions.duplicate-analysis.enable-fingerprint-overlap-detection
        // (default: off).
        let enable_fp = ctx
            .snapshot
            .config
            .as_deref()
            .is_some_and(|c| c.opinions.duplicate_analysis.enable_fingerprint_overlap_detection);
        if enable_fp {
            spawn.push(Computation::DetectFingerprintOverlaps);
        }
    }

    // Cross-domain: tags OR files
    if run_all || s.touches_any(&[RecomputationScope::TAGS, RecomputationScope::FILES]) {
        spawn.push(Computation::DetectPathTagMismatches);

        // DetectMetadataDuplicates is expensive and gated behind
        // opinions.duplicate-analysis.enable-metadata-duplicate-detection
        // (default: off).
        let enable_metadata_dup = ctx
            .snapshot
            .config
            .as_deref()
            .is_some_and(|c| c.opinions.duplicate_analysis.enable_metadata_duplicate_detection);
        if enable_metadata_dup {
            spawn.push(Computation::DetectMetadataDuplicates);
        }
    }

    // Deploy-sensitive: files OR deploy OR tags (deploy path depends on tags+files)
    if run_all
        || s.touches_any(&[
            RecomputationScope::FILES,
            RecomputationScope::DEPLOY,
            RecomputationScope::TAGS,
        ])
    {
        spawn.push(Computation::DetectDeployConflicts);

        // DetectReleaseOverlaps is expensive (iterates all corpus album dirs)
        // and gated behind opinions.duplicate-analysis.enable-release-overlap-detection
        // (default: off). When enabled, it spawns DeriveCorpusDeployStatus
        // behind a barrier so fresh overlap signals are visible. When disabled,
        // we spawn DeriveCorpusDeployStatus directly — it reads whatever
        // ReleaseOverlap signals last landed (may be stale, that's the
        // sunset trade-off).
        let enable_release_overlap = ctx
            .snapshot
            .config
            .as_deref()
            .is_some_and(|c| c.opinions.duplicate_analysis.enable_release_overlap_detection);
        if enable_release_overlap {
            spawn.push(Computation::DetectReleaseOverlaps);
        } else {
            spawn.push(Computation::DeriveCorpusDeployStatus);
        }
    }

    // Deploy health: tags OR deploy
    if run_all || s.touches_any(&[RecomputationScope::TAGS, RecomputationScope::DEPLOY]) {
        // Spawn DeriveDeployHealthSignals for each configured library
        if let Some(config) = ctx.snapshot.config.as_deref() {
            let library_names = get_configured_library_names(config);
            log_general(format!(
                "[COMPUTE] ScheduleContentAnalysis: libraries_dir={:?}, configured_libraries={:?}",
                config.libraries_dir(),
                library_names,
            ));
            for library_name in library_names {
                let library_root = config.libraries_dir().join(&library_name);
                let corpus_path_prefixes = config.get_corpus_paths_for_library(&library_name);
                log_general(format!(
                    "[COMPUTE] ScheduleContentAnalysis: spawning DeriveDeployHealthSignals for '{}' root={:?} prefixes={:?}",
                    library_name, library_root, corpus_path_prefixes,
                ));
                spawn.push(Computation::DeriveDeployHealthSignals {
                    library_name,
                    library_root,
                    corpus_path_prefixes,
                });
            }
        } else {
            log_general("[COMPUTE] ScheduleContentAnalysis: WARNING - failed to load config, no library health computations spawned");
        }
    }

    // External match derivation: EXTERNAL (new data), TAGS (comparison baseline), FILES (new fingerprints)
    if run_all
        || s.touches_any(&[
            RecomputationScope::EXTERNAL,
            RecomputationScope::TAGS,
            RecomputationScope::FILES,
        ])
    {
        spawn.push(Computation::DeriveExternalMatches);
    }

    // Drain Discogs cache → ledger. Sensible at startup and whenever new
    // external data lands (Discogs fetches set EXTERNAL on `SourceDone` with
    // matched > 0). It is intentionally NOT on the TAGS bucket: most TAGS
    // mutations (manual tag edits, canonicalizations) don't change which
    // Discogs releases map to which inodes, so re-deriving every TAGS cycle
    // was pure waste. Release-packing changes happen on TAGS too, but the
    // ledger is idempotent under PK `(inode, genre_id, source, kind)` and
    // the next EXTERNAL pass (or a manual rerun) will pick them up.
    if run_all || s.contains(RecomputationScope::EXTERNAL) {
        spawn.push(Computation::DeriveDiscogsGenreLedger);
    }

    let total_possible = 13; // approximate total without library-specific ones
    log_general(format!(
        "[COMPUTE] ScheduleContentAnalysis: spawning {} computations (of ~{} possible)",
        spawn.len(),
        total_possible
    ));

    Result::success(
        Computation::ScheduleContentAnalysis { scope: *scope },
        spawn,
    )
}
