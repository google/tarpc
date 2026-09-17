// Copyright 2026 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::{
    context::{self, SpanExt},
    trace::{Context, NoActiveSpan, SamplingDecision, SpanId, TraceId},
};
use opentelemetry::trace::TraceContextExt;
use std::time::Instant;
use tracing_opentelemetry::OpenTelemetrySpanExt;

// The deadline travels with the OpenTelemetry context but is interpreted by tarpc.
struct Deadline(Instant);

impl SpanExt for tracing::Span {
    fn trace_context(&self) -> Result<Context, NoActiveSpan> {
        let context = self.context();
        if context.has_active_span() {
            Ok(Context::from(context.span()))
        } else {
            Err(NoActiveSpan)
        }
    }

    fn deadline(&self) -> Option<Instant> {
        self.context().get::<Deadline>().map(|deadline| deadline.0)
    }

    fn set_context(&self, context: &context::Context) {
        // Explicitly ignore the returned result because it either means that the span has
        // already started, or the Otel layer is not present, so we don't mind if the result
        // is an error we silently ignore.
        let _ = self.set_parent(
            opentelemetry::Context::new()
                .with_remote_span_context(opentelemetry::trace::SpanContext::new(
                    opentelemetry::trace::TraceId::from(context.trace_context.trace_id),
                    opentelemetry::trace::SpanId::from(context.trace_context.span_id),
                    opentelemetry::trace::TraceFlags::from(context.trace_context.sampling_decision),
                    true,
                    opentelemetry::trace::TraceState::default(),
                ))
                .with_value(Deadline(context.deadline)),
        );
    }
}

impl From<opentelemetry::trace::TraceId> for TraceId {
    fn from(trace_id: opentelemetry::trace::TraceId) -> Self {
        Self::from(u128::from_be_bytes(trace_id.to_bytes()))
    }
}

impl From<TraceId> for opentelemetry::trace::TraceId {
    fn from(trace_id: TraceId) -> Self {
        Self::from_bytes(u128::from(trace_id).to_be_bytes())
    }
}

impl From<opentelemetry::trace::SpanId> for SpanId {
    fn from(span_id: opentelemetry::trace::SpanId) -> Self {
        Self::from(u64::from_be_bytes(span_id.to_bytes()))
    }
}

impl From<SpanId> for opentelemetry::trace::SpanId {
    fn from(span_id: SpanId) -> Self {
        Self::from_bytes(u64::from(span_id).to_be_bytes())
    }
}

impl From<opentelemetry::trace::SpanRef<'_>> for Context {
    fn from(span: opentelemetry::trace::SpanRef<'_>) -> Self {
        let otel_ctx = span.span_context();
        Self {
            trace_id: TraceId::from(otel_ctx.trace_id()),
            span_id: SpanId::from(otel_ctx.span_id()),
            sampling_decision: SamplingDecision::from(otel_ctx),
        }
    }
}

impl From<SamplingDecision> for opentelemetry::trace::TraceFlags {
    fn from(decision: SamplingDecision) -> Self {
        match decision {
            SamplingDecision::Sampled => opentelemetry::trace::TraceFlags::SAMPLED,
            SamplingDecision::Unsampled => opentelemetry::trace::TraceFlags::default(),
        }
    }
}

impl From<&opentelemetry::trace::SpanContext> for SamplingDecision {
    fn from(context: &opentelemetry::trace::SpanContext) -> Self {
        if context.is_sampled() {
            SamplingDecision::Sampled
        } else {
            SamplingDecision::Unsampled
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SpanExt;
    use crate::{context, trace};
    use opentelemetry::trace::TracerProvider as _;
    use std::time::{Duration, Instant};
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn current_inherits_request_context() {
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("tarpc-test")));

        tracing::subscriber::with_default(subscriber, || {
            let mut parent = context::current();
            parent.deadline = Instant::now() + Duration::from_secs(30);
            parent.trace_context = trace::Context {
                trace_id: 42.into(),
                span_id: 7.into(),
                sampling_decision: trace::SamplingDecision::Sampled,
            };

            let span = tracing::info_span!("RPC");
            span.set_context(&parent);
            let _entered = span.enter();
            let current = context::current();

            assert_eq!(current.deadline, parent.deadline);
            assert_eq!(
                current.trace_context.trace_id,
                parent.trace_context.trace_id
            );
            assert_eq!(
                current.trace_context.sampling_decision,
                parent.trace_context.sampling_decision
            );
            assert!(!current.trace_context.span_id.is_none());
        });
    }
}
