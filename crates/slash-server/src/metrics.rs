//! Prometheus metrics (spec §7.4). The dominant failure modes of this
//! architecture are silent by construction, so `/metrics` ships from M4
//! rather than being deferred. Only the delivery-inbox metrics exist yet;
//! the invocation/correlation/dispatch metrics are added as M5/M6 land the
//! state they describe.

use prometheus::{
    Encoder, HistogramVec, IntCounterVec, IntGauge, IntGaugeVec, Registry, TextEncoder,
    register_histogram_vec_with_registry, register_int_counter_vec_with_registry,
    register_int_gauge_vec_with_registry, register_int_gauge_with_registry,
};

pub struct Metrics {
    registry: Registry,
    pub webhook_deliveries_total: IntCounterVec,
    pub webhook_handler_seconds: HistogramVec,
    pub deliveries_pending: IntGauge,
    /// Spec §7.4: age of the oldest pending delivery, the inbox-stall alarm.
    /// `0` when the inbox is empty.
    pub deliveries_oldest_pending_age_seconds: IntGauge,
    /// Spec §7.4: invocations currently in each status.
    pub invocations: IntGaugeVec,
    /// Spec §7.4's stuck-invocation alarm: how long the oldest still-
    /// `dispatched` invocation has been waiting for a run id. `0` when none.
    pub invocations_max_dispatched_age_seconds: IntGauge,
    /// Spec §7.4/§10 criterion 2: how each dispatch's run id was ultimately
    /// resolved — `dispatch_response`, `polled`, or `timeout` (never
    /// resolved within the dispatch timeout).
    pub correlation_total: IntCounterVec,
    /// Spec §7.4: dispatches that ended in `dispatch_failed`, by class.
    pub dispatch_failures_total: IntCounterVec,
    pub command_catalog_loads_total: IntCounterVec,
}

impl Metrics {
    /// Registers every metric against a fresh registry. Only fails if two
    /// metrics here were given the same name — a build-time programming
    /// error, not a runtime condition — so callers should treat `Err` as
    /// fatal at startup.
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        let webhook_deliveries_total = register_int_counter_vec_with_registry!(
            "slash_webhook_deliveries_total",
            "Webhook deliveries received, by event and outcome.",
            &["event", "outcome"],
            registry
        )?;

        let webhook_handler_seconds = register_histogram_vec_with_registry!(
            "slash_webhook_handler_seconds",
            "POST /webhook handler latency.",
            &["outcome"],
            registry
        )?;

        let deliveries_pending = register_int_gauge_with_registry!(
            "slash_deliveries_pending",
            "Deliveries currently in the pending state (inbox depth).",
            registry
        )?;

        let deliveries_oldest_pending_age_seconds = register_int_gauge_with_registry!(
            "slash_deliveries_oldest_pending_age_seconds",
            "Age in seconds of the oldest pending delivery (0 when none).",
            registry
        )?;

        let invocations = register_int_gauge_vec_with_registry!(
            "slash_invocations",
            "Invocations currently in each status.",
            &["status"],
            registry
        )?;

        let invocations_max_dispatched_age_seconds = register_int_gauge_with_registry!(
            "slash_invocations_max_dispatched_age_seconds",
            "Age in seconds of the longest-stuck invocation in the dispatched status (0 when none).",
            registry
        )?;

        let correlation_total = register_int_counter_vec_with_registry!(
            "slash_correlation_total",
            "How a dispatch's run id was ultimately resolved: dispatch_response, polled, or timeout.",
            &["path"],
            registry
        )?;

        let dispatch_failures_total = register_int_counter_vec_with_registry!(
            "slash_dispatch_failures_total",
            "Dispatches that ended in dispatch_failed, by failure class.",
            &["class"],
            registry
        )?;

        let command_catalog_loads_total = register_int_counter_vec_with_registry!(
            "slash_command_catalog_loads_total",
            "Command catalog loads by terminal outcome and bounded processing stage.",
            &["outcome", "stage"],
            registry
        )?;

        Ok(Self {
            registry,
            webhook_deliveries_total,
            webhook_handler_seconds,
            deliveries_pending,
            deliveries_oldest_pending_age_seconds,
            invocations,
            invocations_max_dispatched_age_seconds,
            correlation_total,
            dispatch_failures_total,
            command_catalog_loads_total,
        })
    }

    /// Renders the current metrics in Prometheus text exposition format.
    pub fn render(&self) -> String {
        let families = self.registry.gather();
        let mut buf = Vec::new();
        let encoder = TextEncoder::new();
        // `encode` only fails if writing to `buf` fails, which cannot happen
        // for an in-memory Vec.
        let _ = encoder.encode(&families, &mut buf);
        String::from_utf8(buf).unwrap_or_default()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn renders_registered_metrics_in_text_format() {
        let metrics = Metrics::new().unwrap();
        metrics
            .webhook_deliveries_total
            .with_label_values(&["issue_comment", "accepted"])
            .inc();
        metrics.deliveries_pending.set(3);
        metrics
            .command_catalog_loads_total
            .with_label_values(&["unavailable", "directory"])
            .inc();

        let output = metrics.render();
        assert!(output.contains("slash_webhook_deliveries_total"));
        assert!(output.contains("slash_deliveries_pending 3"));
        assert!(output.contains(
            "slash_command_catalog_loads_total{outcome=\"unavailable\",stage=\"directory\"} 1"
        ));
    }
}
