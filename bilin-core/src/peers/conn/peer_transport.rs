use std::time::{Duration, Instant};

use super::peer_conn::PeerConnId;

// Smart paths send a probe every second when the link is healthy. Keep enough
// attempts to detect sustained moderate loss without treating one miss as loss.
const PROBE_WINDOW: usize = 20;
const MIN_PROBES: usize = 4;
const MIN_LOST_PROBES: usize = 2;
const DEGRADED_LOSS_PERCENT: u32 = 10;
const MAX_PROBE_AGE: Duration = Duration::from_secs(12);
const EVALUATE_INTERVAL: Duration = Duration::from_secs(1);
const DOWNGRADE_HOLD: Duration = Duration::from_secs(5);
const RECOVERY_HOLD: Duration = Duration::from_secs(30);
const SWITCH_COOLDOWN: Duration = Duration::from_secs(20);

#[derive(Debug, Default)]
pub(super) struct TransportProbes {
    // Missing RTT represents a failed probe. Echo-only replies supply no
    // sample because they can race the matching Pong on a working link.
    samples: [Option<u32>; PROBE_WINDOW],
    next: usize,
    count: usize,
    last_probe: Option<Instant>,
}

impl TransportProbes {
    pub(super) fn record(&mut self, rtt_us: Option<u32>, now: Instant) {
        if self
            .last_probe
            .is_some_and(|last| now.saturating_duration_since(last) > MAX_PROBE_AGE)
        {
            self.next = 0;
            self.count = 0;
        }
        self.samples[self.next] = rtt_us;
        self.next = (self.next + 1) % PROBE_WINDOW;
        self.count = (self.count + 1).min(PROBE_WINDOW);
        self.last_probe = Some(now);
    }

    // Healthy links are ranked by protocol. When all viable links are
    // degraded, prefer the one with the lowest recent loss and jitter.
    pub(super) fn penalty(&self, now: Instant) -> u32 {
        if self.count < MIN_PROBES {
            return 0;
        }
        if self
            .last_probe
            .is_some_and(|last| now.saturating_duration_since(last) > MAX_PROBE_AGE)
        {
            return u32::MAX;
        }

        let first = (self.next + PROBE_WINDOW - self.count) % PROBE_WINDOW;
        let mut successful = 0usize;
        let mut previous: Option<u32> = None;
        let mut jitter_total = 0u64;
        for rtt in (0..self.count).filter_map(|index| self.samples[(first + index) % PROBE_WINDOW])
        {
            if let Some(previous) = previous {
                jitter_total += previous.abs_diff(rtt) as u64;
            }
            previous = Some(rtt);
            successful += 1;
        }
        let average_jitter = (jitter_total / successful.saturating_sub(1).max(1) as u64) as u32;
        let lost_probes = self.count - successful;
        let loss_percent = (lost_probes * 100 / self.count) as u32;
        if (lost_probes >= MIN_LOST_PROBES && loss_percent >= DEGRADED_LOSS_PERCENT)
            || average_jitter >= 35_000
        {
            1 + loss_percent * 100 + average_jitter / 100
        } else {
            0
        }
    }

    pub(super) fn measured(&self, now: Instant) -> bool {
        self.count >= MIN_PROBES
            && self
                .last_probe
                .is_some_and(|last| now.saturating_duration_since(last) <= MAX_PROBE_AGE)
    }
}

pub(super) fn protocol_priority(protocol: Option<&str>) -> u8 {
    match protocol {
        Some("udp") => 0,
        Some("kcp" | "quic") => 1,
        Some("tcp" | "wss" | "ws") => 2,
        _ => 3,
    }
}

#[derive(Debug, Default)]
pub(super) struct TransportSelection {
    evaluated: Option<Instant>,
    // Recovery needs the same candidate to remain healthy for the full hold.
    // Downgrade waits on degradation of the current path while any better one
    // remains available. Small RTT changes between standbys must not restart it.
    pending: Option<(Option<PeerConnId>, Instant)>,
    switched: Option<Instant>,
}

impl TransportSelection {
    pub(super) fn due(&mut self, now: Instant, current_is_live: bool) -> bool {
        if current_is_live
            && self
                .evaluated
                .is_some_and(|last| now.saturating_duration_since(last) < EVALUATE_INTERVAL)
        {
            return false;
        }
        self.evaluated = Some(now);
        true
    }

    pub(super) fn ready(&mut self, candidate: PeerConnId, recovering: bool, now: Instant) -> bool {
        let hold = if recovering {
            RECOVERY_HOLD
        } else {
            DOWNGRADE_HOLD
        };
        let recovery_target = recovering.then_some(candidate);
        if self
            .switched
            .is_none_or(|last| now.saturating_duration_since(last) >= SWITCH_COOLDOWN)
            && self.pending.is_some_and(|(target, start)| {
                target == recovery_target && now.saturating_duration_since(start) >= hold
            })
        {
            self.on_switch(now);
            return true;
        }
        if self
            .pending
            .is_none_or(|(target, _)| target != recovery_target)
        {
            self.pending = Some((recovery_target, now));
        }
        false
    }

    pub(super) fn clear_pending(&mut self) {
        self.pending = None;
    }

    pub(super) fn on_switch(&mut self, now: Instant) {
        self.pending = None;
        self.switched = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_recent_loss_and_jitter_and_recovers_when_old_samples_expire() {
        let now = Instant::now();
        let mut probes = TransportProbes::default();
        for rtt in [Some(25_000), None, Some(26_000), None] {
            probes.record(rtt, now);
        }
        assert!(probes.penalty(now) > 0);
        for _ in 0..PROBE_WINDOW {
            probes.record(Some(26_000), now);
        }
        assert_eq!(probes.penalty(now), 0);
        for rtt in [25_000, 130_000].into_iter().cycle().take(PROBE_WINDOW) {
            probes.record(Some(rtt), now);
        }
        assert!(probes.penalty(now) > 0);
        assert_eq!(
            probes.penalty(now + MAX_PROBE_AGE + Duration::from_secs(1)),
            u32::MAX
        );
        let next_sample = now + MAX_PROBE_AGE + Duration::from_secs(1);
        probes.record(Some(26_000), next_sample);
        assert!(!probes.measured(next_sample));
        assert_eq!(probes.penalty(next_sample), 0);
    }

    #[test]
    fn isolated_timeout_stays_healthy_but_recurring_ten_percent_loss_degrades() {
        let start = Instant::now();
        let mut probes = TransportProbes::default();
        for second in 0..20 {
            let now = start + Duration::from_secs(second);
            probes.record((second != 10).then_some(30_000), now);
            assert_eq!(probes.penalty(now), 0);
        }
        let now = start + Duration::from_secs(20);
        probes.record(None, now);
        assert!(probes.penalty(now) > 0);
    }

    #[test]
    fn alternating_standbys_do_not_postpone_a_downgrade() {
        let start = Instant::now();
        let quic = PeerConnId::new_v4();
        let kcp = PeerConnId::new_v4();
        let mut selection = TransportSelection::default();
        for second in 0..=DOWNGRADE_HOLD.as_secs() {
            let now = start + Duration::from_secs(second);
            let candidate = if second % 2 == 0 { quic } else { kcp };
            assert_eq!(
                selection.ready(candidate, false, now),
                second == DOWNGRADE_HOLD.as_secs()
            );
        }
    }

    #[test]
    fn alternating_recovery_candidates_each_need_their_own_hold() {
        let start = Instant::now();
        let first = PeerConnId::new_v4();
        let second = PeerConnId::new_v4();
        let mut selection = TransportSelection::default();
        assert!(!selection.ready(first, true, start));
        let change = start + RECOVERY_HOLD - Duration::from_secs(1);
        assert!(!selection.ready(second, true, change));
        assert!(!selection.ready(second, true, start + RECOVERY_HOLD));
        assert!(selection.ready(second, true, change + RECOVERY_HOLD));
    }

    #[test]
    fn requires_a_stable_candidate_but_rechecks_a_closed_path_immediately() {
        let now = Instant::now();
        let quic = PeerConnId::new_v4();
        let udp = PeerConnId::new_v4();
        let mut selection = TransportSelection::default();
        assert!(selection.due(now, true));
        assert!(!selection.due(now, true));
        assert!(selection.due(now, false));
        assert!(!selection.ready(quic, false, now));
        assert!(selection.ready(quic, false, now + DOWNGRADE_HOLD));
        let switched = now + DOWNGRADE_HOLD;
        assert!(!selection.ready(udp, false, switched));
        assert!(!selection.ready(udp, false, switched + DOWNGRADE_HOLD));
        assert!(selection.ready(udp, false, switched + SWITCH_COOLDOWN));
        selection.clear_pending();
        let switched_again = switched + SWITCH_COOLDOWN;
        assert!(!selection.ready(quic, true, switched_again));
        assert!(!selection.ready(quic, true, switched_again + SWITCH_COOLDOWN));
        assert!(selection.ready(quic, true, switched_again + RECOVERY_HOLD));
    }
}
