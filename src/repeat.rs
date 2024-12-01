use actix_web::rt::time;
use policy::RepeatPolicy;
use std::fmt::{Debug, Display};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::misc::HumanReadable;

#[derive(Debug)]
pub struct ExecutionFailure<E>(pub E, pub Duration);

impl<E> Display for ExecutionFailure<E>
where
    E: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let error = &self.0;
        let delay = self.1.prettify();
        write!(
            f,
            "postponing next retry in {delay} due to last error: {error:#?}"
        )
    }
}

/// An executor to repeat a task execution periodically.
///
/// Distinct execution period policies can be set for success and failures executions.
///
/// # Example
/// ```
/// let count = AtomicU8::new(0);
/// let task = || async {
///     let result = count.fetch_add(1, Ordering::Acquire);
///     match result {
///         3, 4, 5, 6 => Err(anyhow!("task failure")),
///         result => Ok(result),
///     }
/// };
///
/// // Execute the task every 10 < x < 15 seconds, x being randomly chosen.
/// let repeat_policy = RangeInterval::from(Duration::seconds(10)..Duration::seconds(15));
///
/// // On failure, use an exponential backoff policy starting at 50ms.
/// // Once 1h delay is reached, retry every 1h.
/// let backoff_policy = ExponentialBackoff::new(Duration::from_millis(50), Duration::from_secs(3600));
///
/// let mut executor = RepeatableExecutor::new(task, repeat_policy, backoff_policy);
/// for _ in 0..5 {
///     match executor.execute_next().await {
///         Ok(result) => println!("successful execution {result}"),
///         Err(ExecutionFailure(error, next_delay)) => println("task will be retried in {next_delay:?}")
///     }
/// }
/// ```
#[derive(Default)]
pub struct RepeatableExecutor<S, RP, BP, I, E, Fn, Fut>
where
    S: Sleeper,
    RP: policy::RepeatPolicy,
    BP: RepeatPolicy,
    Fn: FnMut() -> Fut,
    Fut: Future<Output = Result<I, E>>,
{
    sleeper: S,
    operation: Fn,
    repeat_policy: RP,
    backoff_policy: BP,
    next_interval: Duration,
}

impl<RP, BP, I, E, Fn, Fut> RepeatableExecutor<TokioSleeper, RP, BP, I, E, Fn, Fut>
where
    RP: RepeatPolicy,
    BP: RepeatPolicy,
    Fn: FnMut() -> Fut,
    Fut: Future<Output = Result<I, E>>,
{
    pub fn new(operation: Fn) -> Self {
        RepeatableExecutor {
            sleeper: TokioSleeper::default(),
            operation,
            repeat_policy: RP::default(),
            backoff_policy: BP::default(),
            next_interval: Duration::ZERO,
        }
    }

    #[cfg(test)]
    fn with_stub_sleeper(
        self,
        sleeper: StubSleeper,
    ) -> RepeatableExecutor<StubSleeper, RP, BP, I, E, Fn, Fut> {
        RepeatableExecutor {
            sleeper,
            operation: self.operation,
            repeat_policy: self.repeat_policy,
            backoff_policy: self.backoff_policy,
            next_interval: self.next_interval,
        }
    }
}

impl<S, RP, BP, I, E, Fn, Fut> RepeatableExecutor<S, RP, BP, I, E, Fn, Fut>
where
    S: Sleeper,
    RP: RepeatPolicy,
    BP: RepeatPolicy,
    Fn: FnMut() -> Fut,
    Fut: Future<Output = Result<I, E>>,
{
    pub fn with_repeat_policy(mut self, repeat_policy: RP) -> Self {
        self.repeat_policy = repeat_policy;
        self
    }

    pub fn with_backoff_policy(mut self, backoff_policy: BP) -> Self {
        self.backoff_policy = backoff_policy;
        self
    }

    /// Start next interval sleep time and execute the task.
    pub async fn next(&mut self) -> Result<I, ExecutionFailure<E>> {
        self.sleeper.sleep(self.next_interval).await;
        match (self.operation)().await {
            Ok(result) => {
                self.next_interval = self.repeat_policy.next();
                self.backoff_policy.reset();
                Ok(result)
            }
            Err(error) => {
                self.next_interval = self.backoff_policy.next();
                self.repeat_policy.reset();
                Err(ExecutionFailure(error, self.next_interval))
            }
        }
    }
}

pub trait Sleeper: Default {
    async fn sleep(&self, _duration: Duration) {}
}

#[derive(Default)]
pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    async fn sleep(&self, duration: Duration) {
        time::sleep(duration).await;
    }
}

#[cfg(test)]
#[derive(Default, Clone)]
struct StubSleeper {
    requests: Arc<Mutex<Vec<Duration>>>,
}

#[cfg(test)]
impl Sleeper for StubSleeper {
    async fn sleep(&self, duration: Duration) {
        self.requests.lock().unwrap().push(duration);
    }
}

#[cfg(test)]
impl StubSleeper {
    fn requests(self) -> Vec<Duration> {
        self.requests.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {

    use std::{
        sync::atomic::{AtomicU8, Ordering},
        time::Duration,
    };

    use anyhow::anyhow;
    use tokio::time;

    use crate::repeat::{ExecutionFailure, StubSleeper};

    use super::{
        policy::{ExponentialBackoff, FixedInterval},
        RepeatableExecutor,
    };

    #[tokio::test]
    async fn can_schedule_repeated_successful_tasks() {
        let stub_sleeper = StubSleeper::default();

        // Given successful tasks
        let count = AtomicU8::new(0);
        let task = || async { Ok(count.fetch_add(1, Ordering::Acquire)) };

        let mut executor = RepeatableExecutor::<_, _, _, _, (), _, _>::new(task)
            .with_stub_sleeper(stub_sleeper.clone())
            .with_repeat_policy(FixedInterval::every(Duration::from_millis(500)))
            .with_backoff_policy(ExponentialBackoff::new(Duration::ZERO, Duration::ZERO));

        // When 50 tasks are executed
        for _ in 0..50 {
            match executor.next().await {
                Ok(id) => println!("#{id} ✅"),
                Err(ExecutionFailure((), next_delay)) => {
                    println!("💥 retrying in {next_delay:?}")
                }
            }
        }

        // Then
        let sleep_requests = stub_sleeper.requests();
        assert_eq!(sleep_requests.len(), 50, "50 sleep requests have been made");
        assert_eq!(
            sleep_requests[0],
            Duration::ZERO,
            "first sleep request is 0"
        );
        sleep_requests.iter().skip(1).for_each(|request| {
            assert_eq!(
                request,
                &Duration::from_millis(500),
                "sleep requested 500ms"
            )
        });
    }

    #[tokio::test]
    async fn can_switch_to_backoff_policy_on_error() {
        let stub_sleeper = StubSleeper::default();

        // Given all tasks are succesful except:
        // - task 10 to 20
        // - task 30 to 70
        let count = AtomicU8::new(0);
        let task = || async {
            let id = count.fetch_add(1, Ordering::Acquire);
            match id {
                10..20 => Err(anyhow!("E{id}")),
                30..70 => Err(anyhow!("E{id}")),
                id => Ok(id),
            }
        };

        // Given a repeat interval between 9 and 11 seconds
        let repeat_range = Duration::from_secs(9)..=Duration::from_secs(11);
        // And a backoff policy starting with 100ms up to 1h
        let mut executor = RepeatableExecutor::new(task)
            .with_stub_sleeper(stub_sleeper.clone())
            .with_repeat_policy(FixedInterval::between(repeat_range.clone()))
            .with_backoff_policy(ExponentialBackoff::new(
                Duration::from_millis(100),
                Duration::from_secs(3600),
            ));

        // When 100 tasks are executed
        for _ in 0..100 {
            match executor.next().await {
                Ok(id) => print!("✅#{id}|"),
                Err(ExecutionFailure(error, next_delay)) => {
                    print!("💥{error}→{next_delay:?}|")
                }
            }
        }
        println!();

        // Then
        let sleep_requests = stub_sleeper.requests();
        assert_eq!(
            sleep_requests.len(),
            100,
            "100 sleep requests have been made"
        );
        assert_eq!(
            sleep_requests[0],
            Duration::ZERO,
            "first sleep request is 0"
        );
        sleep_requests.iter().skip(1).take(10).for_each(|request| {
            assert!(
                repeat_range.contains(request),
                "sleep requests [1-10] should be between {repeat_range:?} one was {request:?}"
            )
        });

        let mut last_backoff = Duration::ZERO;
        sleep_requests.iter().skip(11).take(10).for_each(|request| {
            assert!(
                &last_backoff < request,
                "backoff sleep request should greater than last one ({last_backoff:?}) but it was {request:?}"
            );
            last_backoff = request.clone();
        });

        sleep_requests.iter().skip(21).take(10).for_each(|request| {
            assert!(
                repeat_range.contains(request),
                "sleep requests [21-30] should be between {repeat_range:?} but one was {request:?}"
            )
        });

        let mut last_backoff = Duration::ZERO;
        sleep_requests.iter().skip(31).take(15).for_each(|request| {
            assert!(
                &last_backoff < request,
                "backoff sleep request should greater than last one ({last_backoff:?}) but it was {request:?}"
            );
            last_backoff = request.clone();
        });
        sleep_requests.iter().skip(46).take(5).for_each(|request| {
            assert_eq!(
                request,
                &Duration::from_secs(3600),
                "backoff sleep request should be 3600s but it was {request:?}"
            )
        });

        sleep_requests.iter().skip(71).take(30).for_each(|request| {
            assert!(
                repeat_range.contains(request),
                "sleep requests [71-99] should be between {repeat_range:?} one was {request:?}"
            )
        });
    }
}

pub mod policy {
    use core::ops::RangeInclusive;
    use log::warn;
    use rand::Rng;
    use std::{cmp, fmt::Display, time::Duration, u32};

    use crate::misc::HumanReadable;

    pub trait RepeatPolicy: Default + Display + Clone {
        fn next(&mut self) -> Duration {
            Duration::ZERO
        }
        fn reset(&mut self) {}
    }

    #[derive(Clone)]
    pub struct FixedInterval {
        range: RangeInclusive<Duration>,
    }

    impl FixedInterval {
        pub fn every(delay: Duration) -> Self {
            Self {
                range: delay..=delay,
            }
        }

        pub fn between(range: RangeInclusive<Duration>) -> Self {
            Self { range }
        }
    }

    impl Default for FixedInterval {
        fn default() -> Self {
            Self::every(Duration::ZERO)
        }
    }

    impl Display for FixedInterval {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let start = self.range.start().prettify();
            let end = self.range.end().prettify();
            write!(f, "between {start} and {end}")
        }
    }

    impl RepeatPolicy for FixedInterval {
        fn next(&mut self) -> Duration {
            if self.range.start() == self.range.end() {
                self.range.start().clone()
            } else {
                rand::thread_rng().gen_range(self.range.clone())
            }
        }
    }

    #[derive(Clone)]
    pub struct ExponentialBackoff {
        initial_delay: Duration,
        max_delay: Duration,
        attempts: exponential_backoff::IntoIter,
    }

    impl Default for ExponentialBackoff {
        fn default() -> Self {
            Self::new(Duration::from_secs(1), Duration::MAX)
        }
    }

    impl ExponentialBackoff {
        pub fn new(initial_delay: Duration, max_delay: Duration) -> Self {
            if initial_delay > max_delay {
                warn!(
                    r#"Inconsistent backoff policy configuration: the "max_delay" value ({max_delay:?}) should be higher than the "initial_delay" ({initial_delay:?})"#
                )
            }
            let max_delay = cmp::max(initial_delay, max_delay);
            let backoff = exponential_backoff::Backoff::new(u32::MAX, initial_delay, max_delay);
            ExponentialBackoff {
                initial_delay,
                max_delay,
                attempts: backoff.into_iter(),
            }
        }
    }

    impl Display for ExponentialBackoff {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let initial = self.initial_delay.prettify();
            let max = self.max_delay.prettify();
            write!(f, "exponential backoff from {initial} up to {max}")
        }
    }

    impl RepeatPolicy for ExponentialBackoff {
        fn next(&mut self) -> Duration {
            match self.attempts.next() {
                Some(Some(delay)) => delay,
                _ => self.max_delay.clone(),
            }
        }

        fn reset(&mut self) {
            //let backoff =
            //    exponential_backoff::Backoff::new(u32::MAX, self.initial_delay, self.max_delay);
            // self.attempts = backoff.into_iter();
        }
    }

    #[cfg(test)]
    mod tests {
        use std::time::Duration;

        use crate::repeat::policy::{ExponentialBackoff, RepeatPolicy};

        use super::FixedInterval;

        #[test]
        fn fixed_interval_policy_generation() {
            let mut policy = FixedInterval::every(Duration::from_secs(10));

            assert_eq!(policy.next(), Duration::from_secs(10))
        }

        #[test]
        fn random_interval_policy_generation() {
            let accepted_range = Duration::from_secs(90)..=Duration::from_secs(110);
            let mut policy = FixedInterval::between(accepted_range.clone());

            for _ in 0..100 {
                let next = policy.next();
                assert!(
                    accepted_range.contains(&next),
                    "{next:?} should be within {accepted_range:?}"
                )
            }
        }

        #[test]
        fn exponential_backoff_policy_generation() {
            let mut policy =
                ExponentialBackoff::new(Duration::from_millis(100), Duration::from_secs(180));
            let mut previous = policy.next();
            for _ in 0..=9 {
                let next = policy.next();
                assert!(previous < next, "Each backoff iteration increase delay");
                previous = next;
            }
            assert_eq!(
                previous,
                Duration::from_secs(180),
                "backoff interations never exceeds 'max_delay'"
            );
            assert_eq!(
                previous,
                Duration::from_secs(180),
                "backoff interations never exceeds 'max_delay'"
            );
            assert_eq!(
                previous,
                Duration::from_secs(180),
                "backoff interations never exceeds 'max_delay'"
            );
        }
    }
}
