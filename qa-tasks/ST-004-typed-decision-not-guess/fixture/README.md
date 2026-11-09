# Legacy service

This service is on the deprecation list. Its replacement (`svc2`) has been in
production for eight months and covers every endpoint this one serves.
No callers remain: the last two internal callers were migrated to `svc2` in
release 24.9, and that release is deployed in every environment.
