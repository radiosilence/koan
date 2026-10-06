- Documented `auth_enabled` default was wrong in four places (it defaults to **true**), and the v0.22.0
  changelog entry contradicted itself. The guides that show how to disable auth now warn that
  doing so leaves the API open to anything that can reach the port.
