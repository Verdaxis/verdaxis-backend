# Phase 5 source-rule checklist

Inventory only. These are source references, not executed Rust test results.
Each compliance definition is listed once. Parametrization is not a definition count.
Reference rows inform the pure helper interface; route input policy is specified separately.
Existing board, price, news, MCP and voyage suites remain regressions, not new ports.

| ID | Source | Rule | Planned scenario | Use |
| --- | --- | --- | --- | --- |
| P5-001 | tests/unit/test_compliance_scoring.py:20 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-002 | tests/unit/test_compliance_scoring.py:35 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-003 | tests/unit/test_compliance_scoring.py:49 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-004 | tests/unit/test_compliance_scoring.py:60 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-005 | tests/unit/test_compliance_scoring.py:71 | Scoring, units, category values and explicit assumptions | fueleu_boundary_equivalence | check |
| P5-006 | tests/unit/test_compliance_scoring.py:86 | Scoring, units, category values and explicit assumptions | fueleu_boundary_equivalence | check |
| P5-007 | tests/unit/test_compliance_scoring.py:98 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-008 | tests/unit/test_compliance_scoring.py:109 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-009 | tests/unit/test_compliance_scoring.py:130 | Scoring, units, category values and explicit assumptions | compliance_score_cases | reference |
| P5-010 | tests/unit/test_compliance_scoring.py:138 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-011 | tests/unit/test_compliance_scoring.py:153 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-012 | tests/unit/test_compliance_scoring.py:164 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-013 | tests/unit/test_compliance_scoring.py:174 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-014 | tests/unit/test_compliance_scoring.py:185 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-015 | tests/unit/test_compliance_scoring.py:193 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-016 | tests/unit/test_compliance_scoring.py:201 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-017 | tests/unit/test_compliance_scoring.py:215 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-018 | tests/unit/test_compliance_scoring.py:218 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-019 | tests/unit/test_compliance_scoring.py:221 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-020 | tests/unit/test_compliance_scoring.py:224 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-021 | tests/unit/test_compliance_scoring.py:227 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-022 | tests/unit/test_compliance_scoring.py:230 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-023 | tests/unit/test_compliance_scoring.py:236 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-024 | tests/unit/test_compliance_scoring.py:246 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-025 | tests/unit/test_compliance_scoring.py:262 | Scoring, units, category values and explicit assumptions | compliance_score_cases | reference |
| P5-026 | tests/unit/test_compliance_scoring.py:273 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-027 | tests/unit/test_compliance_scoring.py:285 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-028 | tests/unit/test_compliance_scoring.py:299 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-029 | tests/unit/test_compliance_scoring.py:311 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-030 | tests/unit/test_compliance_scoring.py:325 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-031 | tests/unit/test_compliance_scoring.py:342 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-032 | tests/unit/test_compliance_scoring.py:353 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-033 | tests/unit/test_compliance_scoring.py:370 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-034 | tests/unit/test_compliance_scoring.py:381 | Scoring, units, category values and explicit assumptions | fueleu_boundary_equivalence | check |
| P5-035 | tests/unit/test_compliance_scoring.py:399 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-036 | tests/unit/test_compliance_scoring.py:408 | Scoring, units, category values and explicit assumptions | compliance_score_cases | reference |
| P5-037 | tests/unit/test_compliance_scoring.py:417 | Scoring, units, category values and explicit assumptions | compliance_fuels_cases | check |
| P5-038 | tests/unit/test_compliance_scoring.py:435 | Scoring, units, category values and explicit assumptions | fueleu_boundary_equivalence | check |
| P5-039 | tests/unit/test_compliance_scoring.py:440 | Scoring, units, category values and explicit assumptions | fueleu_boundary_equivalence | check |
| P5-040 | tests/unit/test_compliance_scoring.py:450 | Scoring, units, category values and explicit assumptions | compliance_score_cases | check |
| P5-041 | tests/unit/test_compliance_pricing.py:44 | Overlay projection, nullable values and calculation assumptions | compliance_pricing_cases | check |
| P5-042 | tests/unit/test_compliance_pricing.py:58 | Overlay projection, nullable values and calculation assumptions | compliance_pricing_cases | check |
| P5-043 | tests/unit/test_compliance_pricing.py:71 | Overlay projection, nullable values and calculation assumptions | compliance_pricing_cases | check |
| P5-044 | tests/unit/test_compliance_pricing.py:85 | Overlay projection, nullable values and calculation assumptions | compliance_pricing_cases | check |
| P5-045 | tests/unit/test_compliance_pricing.py:100 | Overlay projection, nullable values and calculation assumptions | fueleu_boundary_equivalence | check |
| P5-046 | tests/unit/test_compliance_pricing.py:104 | Overlay projection, nullable values and calculation assumptions | compliance_pricing_cases | check |
| P5-047 | tests/unit/test_compliance_pricing.py:110 | Overlay projection, nullable values and calculation assumptions | compliance_pricing_cases | check |
| P5-048 | tests/unit/test_compliance_pricing.py:121 | Overlay projection, nullable values and calculation assumptions | compliance_pricing_cases | check |
| P5-049 | tests/unit/test_compliance_pricing.py:132 | Overlay projection, nullable values and calculation assumptions | compliance_pricing_cases | check |
| P5-050 | tests/unit/test_compliance_pricing.py:336 | Overlay projection, nullable values and calculation assumptions | compliance_overlay_cases | check |
| P5-051 | tests/unit/test_compliance_pricing.py:345 | Overlay projection, nullable values and calculation assumptions | compliance_overlay_cases | check |
| P5-052 | tests/unit/test_compliance_pricing.py:357 | Overlay projection, nullable values and calculation assumptions | compliance_overlay_cases | check |
| P5-053 | tests/unit/test_compliance_pricing.py:369 | Overlay projection, nullable values and calculation assumptions | compliance_overlay_cases | check |
| P5-054 | tests/unit/test_compliance_pricing.py:412 | Overlay projection, nullable values and calculation assumptions | compliance_overlay_cases | check |
| P5-055 | tests/unit/test_compliance_pricing.py:470 | Overlay projection, nullable values and calculation assumptions | compliance_overlay_cases | check |
| P5-056 | tests/unit/test_compliance_pricing.py:495 | Overlay projection, nullable values and calculation assumptions | compliance_overlay_cases | check |
| P5-057 | tests/unit/test_compliance_pricing.py:538 | Overlay projection, nullable values and calculation assumptions | compliance_overlay_cases | check |
| P5-058 | tests/unit/test_compliance_pricing.py:548 | Overlay projection, nullable values and calculation assumptions | compliance_overlay_cases | check |
| P5-059 | tests/unit/test_compliance_pricing.py:561 | Overlay projection, nullable values and calculation assumptions | compliance_pricing_cases | check |
| P5-060 | tests/unit/test_availability_schemas.py:9 | Response fields, optional values and source types | availability_evidence_cases | check |
| P5-061 | tests/unit/test_availability_schemas.py:24 | Response fields, optional values and source types | availability_evidence_cases | check |
| P5-062 | tests/unit/test_demand_schemas.py:10 | Response fields, optional values and source types | demand_evidence_cases | check |
| P5-063 | tests/unit/test_demand_schemas.py:24 | Response fields, optional values and source types | demand_evidence_cases | check |
| P5-064 | tests/unit/test_producer_model.py:7 | Response fields, optional values and source types | producer_projection_cases | check |
| P5-065 | tests/unit/test_producer_model.py:18 | Response fields, optional values and source types | producer_projection_cases | check |
| P5-066 | tests/unit/test_producer_schemas.py:11 | Response fields, optional values and source types | producer_projection_cases | check |
| P5-067 | tests/unit/test_producer_schemas.py:28 | Response fields, optional values and source types | producer_projection_cases | check |
| P5-068 | tests/unit/test_public_market_catalog_contract.py:19 | Shared catalog, evidence and current-owner selection | availability_evidence_cases | check |
| P5-069 | tests/unit/test_public_market_catalog_contract.py:45 | Shared catalog, evidence and current-owner selection | availability_evidence_cases | check |
| P5-070 | tests/unit/test_public_market_catalog_contract.py:95 | Shared catalog, evidence and current-owner selection | availability_evidence_cases | check |
| P5-071 | tests/unit/test_public_market_catalog_contract.py:125 | Shared catalog, evidence and current-owner selection | availability_evidence_cases | check |
| P5-072 | tests/unit/test_market_evidence_policy.py:35 | Shared catalog, evidence and current-owner selection | demand_evidence_cases | check |
| P5-073 | tests/unit/test_market_evidence_policy.py:59 | Shared catalog, evidence and current-owner selection | demand_evidence_cases | check |
| P5-074 | tests/unit/test_market_evidence_policy.py:100 | Shared catalog, evidence and current-owner selection | demand_evidence_cases | check |
| P5-075 | tests/unit/test_market_evidence_policy.py:140 | Shared catalog, evidence and current-owner selection | demand_evidence_cases | check |
| P5-076 | tests/unit/test_market_evidence_policy.py:152 | Shared catalog, evidence and current-owner selection | demand_evidence_cases | check |
| P5-077 | tests/unit/test_market_evidence_policy.py:164 | Shared catalog, evidence and current-owner selection | demand_evidence_cases | check |
| P5-078 | tests/unit/test_market_evidence_policy.py:256 | Shared catalog, evidence and current-owner selection | demand_evidence_cases | check |
| P5-079 | tests/unit/test_public_owner_admission.py:283 | Shared catalog, evidence and current-owner selection | compliance_overlay_cases | check |
| P5-080 | tests/unit/test_public_owner_admission.py:319 | Shared catalog, evidence and current-owner selection | compliance_overlay_cases | check |
| P5-081 | tests/unit/test_public_owner_admission.py:342 | Shared catalog, evidence and current-owner selection | compliance_overlay_cases | check |
| P5-082 | tests/unit/test_public_owner_admission.py:505 | Shared catalog, evidence and current-owner selection | compliance_overlay_cases | check |
| P5-083 | tests/unit/test_public_owner_admission.py:550 | Shared catalog, evidence and current-owner selection | compliance_overlay_cases | check |
| P5-084 | tests/unit/test_public_owner_admission.py:631 | Shared catalog, evidence and current-owner selection | compliance_overlay_cases | check |
| P5-085 | tests/unit/test_public_owner_admission.py:670 | Shared catalog, evidence and current-owner selection | compliance_overlay_cases | check |
| P5-086 | tests/unit/test_public_owner_admission.py:689 | Shared catalog, evidence and current-owner selection | compliance_overlay_cases | check |

Shared interfaces: current actor transaction, public eligible-order policy, catalog lanes, exact decimals and captured reporting year.
No operator implementation or broad test-suite parity gate is part of this checklist.
