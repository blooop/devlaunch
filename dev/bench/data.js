window.BENCHMARK_DATA = {
  "lastUpdate": 1786743479141,
  "repoUrl": "https://github.com/blooop/devlaunch",
  "entries": {
    "devlaunch launch stages": [
      {
        "commit": {
          "author": {
            "name": "Austin Gregg-Smith",
            "username": "blooop",
            "email": "blooop@gmail.com"
          },
          "committer": {
            "name": "GitHub",
            "username": "web-flow",
            "email": "noreply@github.com"
          },
          "id": "451b2824babfcb190cf3aebcc8d137b2d4bcdf3a",
          "message": "Merge pull request #205 from blooop/wayfinder/devlaunch-bench-reset-script\n\nbench: put the cold reset in a script pixi's task shell cannot re-parse",
          "timestamp": "2026-08-14T21:25:46Z",
          "url": "https://github.com/blooop/devlaunch/commit/451b2824babfcb190cf3aebcc8d137b2d4bcdf3a"
        },
        "date": 1786743174776,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.260593,
            "range": "± 0.062043",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.316873,
            "range": "± 0.006671",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000608,
            "range": "± 9.1e-05",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.577569,
            "range": "± 0.058759",
            "unit": "s",
            "extra": "runs=5/5 wall=1.687662s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.17127,
            "range": "± 0.060149",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.841949,
            "range": "± 0.503519",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.376468,
            "range": "± 0.051533",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.177795,
            "range": "± 0.118466",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.56797,
            "range": "± 0.619607",
            "unit": "s",
            "extra": "runs=5/5 wall=10.673897s v0.26.1, Linux-X64"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "Austin Gregg-Smith",
            "username": "blooop",
            "email": "blooop@gmail.com"
          },
          "committer": {
            "name": "GitHub",
            "username": "web-flow",
            "email": "noreply@github.com"
          },
          "id": "451b2824babfcb190cf3aebcc8d137b2d4bcdf3a",
          "message": "Merge pull request #205 from blooop/wayfinder/devlaunch-bench-reset-script\n\nbench: put the cold reset in a script pixi's task shell cannot re-parse",
          "timestamp": "2026-08-14T21:25:46Z",
          "url": "https://github.com/blooop/devlaunch/commit/451b2824babfcb190cf3aebcc8d137b2d4bcdf3a"
        },
        "date": 1786743316600,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 0.970484,
            "range": "± 0.057505",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.221477,
            "range": "± 0.004757",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000723,
            "range": "± 0.00013",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.193377,
            "range": "± 0.059905",
            "unit": "s",
            "extra": "runs=5/5 wall=1.312675s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 0.929991,
            "range": "± 0.117798",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.175593,
            "range": "± 0.042398",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.230405,
            "range": "± 0.008125",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.940905,
            "range": "± 0.148331",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.29324,
            "range": "± 0.197147",
            "unit": "s",
            "extra": "runs=5/5 wall=9.417492s v0.26.1, Linux-X64"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "Austin Gregg-Smith",
            "username": "blooop",
            "email": "blooop@gmail.com"
          },
          "committer": {
            "name": "GitHub",
            "username": "web-flow",
            "email": "noreply@github.com"
          },
          "id": "451b2824babfcb190cf3aebcc8d137b2d4bcdf3a",
          "message": "Merge pull request #205 from blooop/wayfinder/devlaunch-bench-reset-script\n\nbench: put the cold reset in a script pixi's task shell cannot re-parse",
          "timestamp": "2026-08-14T21:25:46Z",
          "url": "https://github.com/blooop/devlaunch/commit/451b2824babfcb190cf3aebcc8d137b2d4bcdf3a"
        },
        "date": 1786743478666,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.062208,
            "range": "± 0.072889",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.287913,
            "range": "± 0.027977",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000761,
            "range": "± 5.7e-05",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.352665,
            "range": "± 0.098746",
            "unit": "s",
            "extra": "runs=5/5 wall=1.47404s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.070026,
            "range": "± 0.062937",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.391234,
            "range": "± 0.420437",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.17157,
            "range": "± 0.017001",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.191721,
            "range": "± 0.192367",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.801629,
            "range": "± 0.450774",
            "unit": "s",
            "extra": "runs=5/5 wall=9.92642s v0.26.1, Linux-X64"
          }
        ]
      }
    ]
  }
}