# Probe — Thaw Table and ProgressBar attribute forwarding

HEAD: d58a94f
bundle sha256: 1332238b5efa

## Raw evaluate output

```json
{
  "table": {
    "found": true,
    "tagName": "TABLE",
    "classList": [
      "thaw-table"
    ],
    "cellCount": 1
  },
  "progress": {
    "found": true,
    "tagName": "DIV",
    "role": "progressbar",
    "ariaLabel": "probe"
  }
}
```

## Verdicts

probe-table: landed
probe-progress: landed
probe-progress-label: landed
