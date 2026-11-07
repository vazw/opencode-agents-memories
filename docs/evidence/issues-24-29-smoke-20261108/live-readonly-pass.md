# Live 3742 GET-only read-only pass (issues 24+29 wave, 2026-11-07T19:37:31Z)

Base: http://127.0.0.1:3742. Token: length + 4-char prefix only, never the value. The store is never mutated: every call below is a GET, enforced here and re-checked at exit against this run log.

| row | GET | code | excerpt |
|---|---|---|---|
| health | /api/health | 200 | {"success":true,"status":"ok","authEnabled":false} |
| tags | /api/tags | 200 | {"success":true,"data":{"project":[{"tag":"opencode_project_03b7931d97d0837c","displayName":"/home/vaz/Code/Personal/VPN |
| memories | /api/memories?page=1 | 200 | {"success":true,"data":{"items":[{"type":"memory","id":"auto-capture-36413315eba1bf052eeafe650e3167b370bd6a81c0c54def975 |
| stats | /api/stats | 200 | {"success":true,"data":{"total":5382,"byScope":{"user":0,"project":5382},"byType":{"action":1,"adr":1,"advisory":1,"amen |
| user-profile | /api/user-profile | 200 | {"success":true,"data":{"exists":true,"id":"profile_1791655775197_i8ytfa1","userId":"vaz@siamstr.com","displayName":"vaz |
| user-profiles | /api/user-profiles | 200 | {"success":true,"data":[{"userId":"vaz@siamstr.com","displayName":"vazw","version":181,"totalPromptsAnalyzed":1430,"last |
| projects | /api/projects | 200 | {"success":true,"data":[{"projectId":"/home/vaz/Code/Personal/Rust/opencode-agents-memory","sessionCount":347,"milestone |
| skills-catalog | /skills/index.json | 200 | {"skills":[]} |
| search | /api/search?q=probe | 200 | {"success":true,"data":{"items":[{"type":"prompt","id":"prompt_1793311397853_hvz3eh2","sessionId":"ses_e90309e58ffeUH4Xe |
| sessions | /api/projects/%2Fhome%2Fvaz%2FCode%2FPersonal%2FRust%2Fopencode-agents-memory/sessions | 200 | {"success":true,"data":[{"agentId":"syseng","agentName":"syseng","createdAt":1794078421632,"filesTouched":["crates/mem-w |
| milestones | /api/projects/%2Fhome%2Fvaz%2FCode%2FPersonal%2FRust%2Fopencode-agents-memory/milestones | 200 | {"success":true,"data":[]} |
| catalog-file | - | n/a | live catalog has no skill files |

## Populated arms

- memories rows: 20 (gate: > 0)
- search: q=probe (code and hits in the table above)
- project roster: populated (id resolved, sessions/milestones answered)
