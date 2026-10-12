use std::num::NonZeroUsize;
use std::sync::Arc;

use server::job::{JobError, JobKind, JobRegistry};

#[tokio::test]
async fn a_panicking_job_does_not_poison_the_pool_or_leak_its_registry_entry() {
    let registry =
        JobRegistry::new(NonZeroUsize::new(2).expect("2 is nonzero")).expect("worker pool starts");

    let panicked = registry
        .submit(JobKind::Anonymize, 0, || -> u32 { panic!("boom") })
        .await;

    assert!(matches!(panicked, Err(JobError::WorkerLost)));
    assert!(
        registry.list().is_empty(),
        "the registry entry must be evicted"
    );

    let recovered = registry
        .submit(JobKind::Anonymize, 0, || 1 + 1)
        .await
        .expect("the pool must survive a prior panic");

    assert_eq!(recovered, 2);
    assert!(registry.list().is_empty());
}

#[tokio::test]
async fn submitting_past_capacity_is_rejected() {
    let registry = Arc::new(
        JobRegistry::new(NonZeroUsize::new(1).expect("1 is nonzero")).expect("worker pool starts"),
    );

    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let occupier = tokio::spawn({
        let registry = Arc::clone(&registry);
        async move {
            let _ = registry
                .submit(JobKind::Anonymize, 0, move || {
                    let _ = release_rx.recv();
                })
                .await;
        }
    });

    for _ in 0..10_000 {
        if !registry.list().is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(registry.list().len(), 1);

    let saturated = registry.submit(JobKind::Anonymize, 0, || ()).await;
    assert!(matches!(saturated, Err(JobError::Saturated)));

    release_tx.send(()).expect("release the occupier");
    occupier.await.expect("occupier task completes");
    assert!(registry.list().is_empty());
}

#[tokio::test]
async fn cancelling_the_caller_does_not_free_a_slot_the_job_still_occupies() {
    let registry = Arc::new(
        JobRegistry::new(NonZeroUsize::new(1).expect("1 is nonzero")).expect("worker pool starts"),
    );

    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let submitter = tokio::spawn({
        let registry = Arc::clone(&registry);
        async move {
            let _ = registry
                .submit(JobKind::Anonymize, 0, move || {
                    let _ = release_rx.recv();
                })
                .await;
        }
    });

    for _ in 0..10_000 {
        if !registry.list().is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        registry.list().len(),
        1,
        "the job must be admitted before it is cancelled"
    );

    // Simulates a client disconnect or a timeout: the future awaiting the
    // job's result is dropped while the job itself is still running.
    submitter.abort();
    let _ = submitter.await;

    assert_eq!(
        registry.list().len(),
        1,
        "cancelling the caller must not evict a job that is still running"
    );
    let saturated = registry.submit(JobKind::Anonymize, 0, || ()).await;
    assert!(
        matches!(saturated, Err(JobError::Saturated)),
        "a cancelled job must still count against admission"
    );

    release_tx.send(()).expect("release the job");
    for _ in 0..10_000 {
        if registry.list().is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        registry.list().is_empty(),
        "the job must evict itself once it completes"
    );
}
