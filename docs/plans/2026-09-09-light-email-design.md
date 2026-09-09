# Verdaxis light-mode email design

**Goal:** Apply the Verdaxis light-mode design to all seven automated email types.

**Design:** Use the existing horizontal logo, #F8FAFC page background, white panel, #E2E8F0 borders, #334155 text, #64748B supporting text, and #5DADE2 buttons with dark text. Use a single readable column, restrained brand-blue labels, and a pale application-details table. Account approval copy welcomes the recipient without claiming organization or membership approval.

**Implementation:** Keep delivery and frozen approval payload behavior intact. Replace duplicated HTML with one shared renderer in app/services/email.py. Use inline styles and presentation tables for email clients, a hidden preview line, responsive padding, and fallback action links. Existing public PNG logo remains the only image. No dependencies or frontend changes.

**Checks:** Cover all seven render paths, escaped content, action URLs, expiry text, and unchanged delivery contracts. Render desktop/mobile samples with images enabled/blocked and inspect both requested previews. Send the two refreshed test previews to admin@verdaxis.exchange. Run branch CI, mandatory deployment dry runs, and deploy staging then production with unchanged migration checkpoints.

**Copy and assistance:** Account approval links directly to /app/marketplace, with the existing login/onboarding gates. All new emails freeze Reply-To as admin@verdaxis.exchange and include a mailto support link. Existing frozen approval payloads retain their original fields during retries.
