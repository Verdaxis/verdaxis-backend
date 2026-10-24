# Phase 5 deployment notes

Route composition in source does not authorize live routing or deployment.
Use the reviewed deployment/checkpoint procedure and separate operator approval.

## Fleet artifact publication

The fleet document must be published only by atomic rename, not by in-place
writes. Write the complete document to a separate regular file in the trusted
data directory, then rename it over the configured target. Same-size in-place
rewrites inside timestamp resolution can escape the metadata stability check.
The reader can serve a stable old opened file after a rename; a concurrent
metadata change can conservatively return unavailable. The reader never certifies
publisher provenance. Missing or unavailable fleet data does not change database
readiness.
