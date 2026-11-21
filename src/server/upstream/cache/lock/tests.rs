use super::*;

fn lock_config(timeout: Duration, age: Duration) -> ProxyCacheLockConfig {
    ProxyCacheLockConfig {
        enabled: true,
        timeout,
        age,
    }
}

#[tokio::test]
async fn dropping_the_guard_releases_the_cache_lock() {
    let locks = CacheLocks::default();
    let config = lock_config(Duration::from_millis(50), Duration::from_secs(1));

    let CacheLockOutcome::Acquired(guard) = locks.acquire("key", &config).await else {
        panic!("first cache lock acquisition should succeed");
    };
    assert!(locks.is_held("key"));
    drop(guard);
    assert!(!locks.is_held("key"));

    let CacheLockOutcome::Acquired(guard) = locks.acquire("key", &config).await else {
        panic!("released cache lock should be acquirable again");
    };
    drop(guard);
    assert!(!locks.is_held("key"));
}

#[tokio::test]
async fn expired_holder_takeover_cannot_be_released_by_the_old_guard() {
    let locks = Arc::new(CacheLocks::default());
    let config = lock_config(Duration::from_secs(1), Duration::from_millis(10));
    let CacheLockOutcome::Acquired(old_guard) = locks.acquire("key", &config).await else {
        panic!("first cache lock acquisition should succeed");
    };

    let takeover_locks = locks.clone();
    let takeover_config = config.clone();
    let takeover =
        tokio::spawn(async move { takeover_locks.acquire("key", &takeover_config).await });
    let CacheLockOutcome::Acquired(new_guard) = takeover.await.unwrap() else {
        panic!("expired cache lock should be taken over");
    };
    assert!(locks.is_held("key"));

    drop(old_guard);
    assert!(
        locks.is_held("key"),
        "the old guard must not release the replacement holder"
    );

    let timed_out = locks
        .acquire(
            "key",
            &lock_config(Duration::from_millis(20), Duration::from_secs(1)),
        )
        .await;
    assert!(matches!(timed_out, CacheLockOutcome::TimedOut));
    assert!(locks.is_held("key"));

    drop(new_guard);
    assert!(!locks.is_held("key"));
}

#[tokio::test]
async fn multiple_waiters_observe_each_release_without_lost_wakeups() {
    let locks = Arc::new(CacheLocks::default());
    let config = lock_config(Duration::from_secs(1), Duration::from_secs(2));
    let CacheLockOutcome::Acquired(owner) = locks.acquire("key", &config).await else {
        panic!("initial cache lock acquisition should succeed");
    };

    let mut waiters = Vec::new();
    for _ in 0..8 {
        let locks = locks.clone();
        let config = config.clone();
        waiters.push(tokio::spawn(async move {
            let CacheLockOutcome::Acquired(guard) = locks.acquire("key", &config).await else {
                return false;
            };
            drop(guard);
            true
        }));
    }

    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(owner);
    for waiter in waiters {
        assert!(
            waiter.await.unwrap(),
            "a waiting client timed out instead of receiving the released lock"
        );
    }
    assert!(!locks.is_held("key"));
}
