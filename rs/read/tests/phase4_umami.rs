//! Task 16 suite entry: the adapter and event-policy modules are included
//! below their own test directory.
//!
//! Every provider destination in this suite is a numeric-loopback HTTP stub.
//! The suite never contacts a real Umami host and never reads or prints a real
//! credential; the test credential is obviously fake.
//!
//! The two analytics modules this task produces are compiled against doubles
//! of the Task 15 types they consume (query, typed diagnostics and cells,
//! suppression policy). The doubles are built strictly from the CF4_03
//! declarations and are replaced by the real producer before acceptance.

pub use verdaxis_read::*;

pub mod analytics {
    pub mod query {
        use chrono::{DateTime, Utc};
        use uuid::Uuid;

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum AnalyticsAudience {
            All,
            Buyer,
            Supplier,
        }

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum AnalyticsActivity {
            Live,
            Demo,
            Reference,
            All,
        }

        /// CF4_03 `AnalyticsQuery`, with the defaults the shared parser applies.
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct AnalyticsQuery {
            pub start: DateTime<Utc>,
            pub end: DateTime<Utc>,
            pub previous: Option<(DateTime<Utc>, DateTime<Utc>)>,
            pub compare: bool,
            pub audience: AnalyticsAudience,
            pub activity: AnalyticsActivity,
            pub product_id: Option<Uuid>,
            pub delivery_point_id: Option<Uuid>,
            pub availability_window: Option<String>,
        }
    }

    pub mod types {
        use chrono::{DateTime, Utc};
        use serde::{Deserialize, Serialize};

        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum AnalyticsDiagnostic {
            Disabled,
            Configuration,
            Authentication,
            Timeout,
            Upstream,
            MalformedResponse,
            InsufficientCoverage,
        }

        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum AnalyticsSourceStatus {
            Available,
            Partial,
            Unavailable,
            NotApplicable,
        }

        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub struct AnalyticsSourceCoverage {
            pub coverage_start: Option<DateTime<Utc>>,
            pub coverage_end: Option<DateTime<Utc>>,
            pub observed_at: Option<DateTime<Utc>>,
            pub status: AnalyticsSourceStatus,
            pub diagnostic: Option<AnalyticsDiagnostic>,
        }

        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub struct AggregateCell {
            pub key: String,
            pub count: Option<u64>,
            pub suppressed: bool,
        }

        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum ComponentStatus {
            Available,
            Partial,
            Unavailable,
            NotRequested,
        }

        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub struct ProviderComponent<T> {
            pub status: ComponentStatus,
            pub diagnostic: Option<AnalyticsDiagnostic>,
            pub data: Option<T>,
        }
    }

    pub mod policy {
        use crate::analytics::types::AggregateCell;

        /// CF4_03 `suppress`: a positive count or entity basis of 1 or 2
        /// suppresses the count; zero stays a genuine zero.
        pub fn suppress(count: Option<u64>, entities: Option<u64>) -> AggregateCell {
            let basis = entities.or(count);
            let suppressed = matches!(basis, Some(1 | 2));
            AggregateCell {
                key: String::new(),
                count: if suppressed { None } else { count },
                suppressed,
            }
        }
    }

    pub mod events {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/analytics/events.rs"));
    }
    pub mod behavioral {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/analytics/behavioral.rs"));
    }
}

pub mod providers {
    pub mod umami {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/providers/umami.rs"));
    }
}

#[path = "phase4_umami_suite/cases.rs"]
mod cases;
