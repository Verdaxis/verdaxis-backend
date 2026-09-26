# B30 and B100 markets: research and contract decisions

Research checked: 26 September 2026.

B30 and B100 use the existing exchange orderbook as separate products. Each initial contract supports one defined fuel profile so buyers and suppliers trade the same product and each ticker and forward curve describes one grade.

## Supported B30 contract

| Field | Requirement |
| --- | --- |
| Market code and name | `B30` |
| Product UUID | `c4cecebc-3d2d-5840-8021-57a9a11bc673` |
| Composition | 30% FAME by volume and 70% VLSFO |
| Finished-fuel specification | ISO 8217:2024 RF 380 |
| Stored specification string | `ISO 8217:2024 RF 380` |
| Finished-fuel sulphur | At most 0.50% by mass |
| Trading unit | Metric tonnes, with the existing 200 MT minimum lot |

The composition and sulphur limit are part of the canonical product contract, even though the stored specification field contains only the standard and grade. Both sides must see this definition. Supplier declarations must describe the finished blend.

This is a consciously narrower first exchange contract, not a definition of all fuels sold as B30. DNV reports an established Rotterdam B30 market, mainly using FAME and VLSFO. However, GoodFuels has also supplied B30 made from HVO and MGO. These compositions must not share an automatic matching book. [DNV market overview](https://www.dnv.com/expert-story/maritime-impact/maximizing-the-potential-of-biofuels-in-shipping/), [GoodFuels HVO/MGO delivery](https://www.goodfuels.com/news/gcmds-third-biofuel-supply-chain-trial-involves-inline-tracer-dosing-and-onboard-blending)

RF 380 is the bio-residual grade designation; RMG 380 is a different grade. The CIMAC/ISO working-group FAQ explains that RF 380 has a minimum viscosity of 120 mm²/s at 50°C. It also explains why RF 80 exists for lower-viscosity biofuels. RF 80 and other RF grades can be valid marine fuels, but are unsupported by this book. Selecting RF 380 is an exchange scope decision, not a claim that all B30 meets it. [CIMAC FAQ, section 2.25](https://www.cimac.com/cms/upload/workinggroups/WG7/CIMAC_Guideline_ISO_8217_2024_FAQ_02-2024_Rev4.pdf)

## Supported B100 contract

The user requested B100 as well as B30. Define `B100` as neat FAME biodiesel, with no fossil-fuel or HVO blend, meeting finished-fuel `ISO 8217:2024 DFA` and sulphur at most 0.10% by mass. Store exactly `ISO 8217:2024 DFA`. Retain the same trading unit and minimum lot. Neat biodiesel does not mean a laboratory ester-purity requirement of exactly 100%; the standard's quality and impurity limits apply.

DFA is a valid initial marine grade for neat FAME. ISO 8217:2024 DF grades permit up to 100% FAME. The FAME must meet EN 14214 or ASTM D6751 as referenced by ISO 8217, including its stated exceptions; the finished product must also meet the applicable Table 1 grade. EN 14214 alone does not establish the full marine specification. The 0.10% sulphur cap is this exchange contract's explicit limit, not a claim that all DFA has that limit. Other DF grades and HVO are outside this book. [CTI-Maritec technical note](https://www.maritec.com.sg/resources/ck/files/CTIMaritec%20NEWSLETTER_May%202024_Edition%203.pdf), [AGQM 2026 guide, page 2](https://www.agqm-biodiesel.com/application/files/1217/8004/6377/2026_ENG_AGQM_approvals_inland_navigation.pdf), [CIMAC FAQ, sections 2.4 and 2.8](https://www.cimac.com/cms/upload/workinggroups/WG7/CIMAC_Guideline_ISO_8217_2024_FAQ_02-2024_Rev4.pdf)

## Existing trading system

Reuse the current product, port, delivery-window, certification, price and time matching rules, partial fills, order expiry, and trade lifecycle. Apply each fixed fuel specification at listing, update, assisted-order, and execution boundaries. No new trading engine or parser for arbitrary grades is needed.

Filters, map tickers, port lists, and forward curves use the separate canonical B30 and B100 identities. Future demand for RF 80, another base fuel, or another blend can be met with a separate canonical product profile and its own prices. Do not broaden the meaning of an existing contract after orders exist.

The pre-change active catalog has four alcohol contracts and no canonical B100 contract. Legacy/demo B100 content does not establish an existing tradable product; both new products need canonical catalog entries.

## Supplier evidence and carbon data

Technical quality, sustainability certification, and batch evidence serve different purposes. ISO 8217 specifies marine-fuel quality. ISCC EU and ISCC PLUS serve different regulated and voluntary uses; a selected scheme does not establish eligibility for every regulation. [ISO 8217:2024 scope](https://www.iso.org/cms/live/live/en/sites/isoorg/contents/data/standard/08/05/80579.html), [ISCC marine-fuel schemes](https://iscc-system.org/about/markets/alternative-transport-fuels/alternative-marine-fuels/)

Reuse the existing supplier declaration, specification, SDS availability, feedstock, origin, and carbon-intensity fields. The app does not verify supplier certificates or provide a production upload flow for CoQ/CoA or Proof of Sustainability. Supporting quality and sustainability documents must be exchanged through the commercial process before delivery; the UI must not claim they were uploaded or verified. MPA's Singapore framework calls for agreed specifications and a loading-facility Certificate of Quality before delivery. Listing acceptance does not approve a vessel or delivery. [MPA biofuel framework](https://www.mpa.gov.sg/port-marine-ops/marine-services/bunkering/biofuel-bunkering)

B30 carbon intensity must describe the whole delivered blend; B100 needs its own whole-fuel declared intensity and method. B100 does not imply zero lifecycle emissions. Do not copy the FAME component's value, treat 30% volume as 30% energy, or promise a fixed emissions reduction. DNV calls for component information in blend reporting. Keep estimates unavailable when the needed batch data is absent. [DNV FuelEU FAQ](https://www.dnv.com/maritime/insights/topics/fueleu-maritime/faq/)

Demo quotes are indicative demonstration data only. They are not evidence of real B30 prices or a market benchmark. Real curves and prices require actual eligible market data; do not derive them from B100 prices.

## Demo price basis

The initial seed uses ENGINE's 25 September 2026 physical bunker-price snapshot: Singapore B30-VLSFO (UCOME) USD 1,105/MT; Rotterdam B30-VLSFO (POMEME) USD 869/MT; Rotterdam B100 USD 1,362/MT. The public snapshot does not identify the exact RF 380/DFA grades, so these are dated reference proxies for this demonstration, not verified prices for the exchange contracts. Rotterdam B30 includes Dutch credit economics; that discount is not exported to other ports.

Synthetic bid/ask spreads and forward scenarios surround these anchors. Other ports use disclosed demonstration proxies rather than claimed local assessments. All such orders remain DEMO and non-executable; automatic expiry refresh does not turn the source date into a new market observation. No assessed-price feed or continuing price update service is added.

Source: [ENGINE, 25 September 2026](https://www.engine.online/news/biofuel-bunker-snapshot-rotterdams-b30-vlsfo-at-steep-discount-to-antwerps-blend-81dc). The B100 observation does not state its feedstock. No calorific-value-adjusted or compliance-adjusted USD/VLSFO-equivalent price is used as USD per physical tonne.
