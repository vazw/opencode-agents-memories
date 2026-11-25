# Live 3742 GET-only read-only pass (issues 24+29 wave, 2026-11-25T03:56:12Z)

Base: http://127.0.0.1:3749. Token: length + 4-char prefix only, never the value. The store is never mutated: every call below is a GET, enforced here and re-checked at exit against this run log.

| row | GET | code | excerpt |
|---|---|---|---|
| health | /api/health | 200 | {"success":true,"status":"ok","authEnabled":false} |
| tags | /api/tags | 200 | {"success":true,"data":{"project":[{"tag":"opencode_project_2416a5f68e61f821","displayName":"/home/opencode/.config/open |
| memories | /api/memories?page=1 | 200 | {"success":true,"data":{"items":[{"type":"memory","id":"auto-capture-ad35568fa966792ce97a4dd04dfc0e3a43f824a77e8cd0e6cb4 |
| stats | /api/stats | 200 | {"success":true,"data":{"total":5602,"byScope":{"user":0,"project":5602},"byType":{"action":1,"adr":1,"advisory":1,"amen |
| user-profile | /api/user-profile | 200 | {"success":true,"data":{"exists":false,"userId":"vaz@siamstr.com","message":"No profile found. Keep chatting to build yo |
| user-profiles | /api/user-profiles | 200 | {"success":true,"data":[]} |
| projects | /api/projects | 200 | {"success":true,"data":[{"projectId":"/home/opencode/Code/Work/antwall","sessionCount":223,"milestoneCount":0,"lastActiv |
| skills-catalog | /skills/index.json | 200 | {"skills":[]} |
| search | /api/search?q=probe | 200 | {"success":true,"data":{"items":[{"type":"prompt","id":"prompt_1793311397853_hvz3eh2","sessionId":"ses_e90309e58ffeUH4Xe |
| sessions | /api/projects/%2Fhome%2Fopencode%2FCode%2FWork%2Fantwall/sessions | 200 | {"success":true,"data":[{"agentId":"syseng","agentName":"syseng","createdAt":1795573785931,"filesTouched":[".tmp/drill-b |
| milestones | /api/projects/%2Fhome%2Fopencode%2FCode%2FWork%2Fantwall/milestones | 200 | {"success":true,"data":[]} |
| catalog-file | - | n/a | live catalog has no skill files |

## Populated arms

- memories rows: 20 (gate: > 0)
- search: q=probe (code and hits in the table above)
- project roster: populated (id resolved, sessions/milestones answered)
