window.BENCHMARK_DATA = {
  "lastUpdate": 1786753516354,
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
      },
      {
        "commit": {
          "author": {
            "email": "blooop@gmail.com",
            "name": "Austin Gregg-Smith",
            "username": "blooop"
          },
          "committer": {
            "email": "noreply@github.com",
            "name": "GitHub",
            "username": "web-flow"
          },
          "distinct": true,
          "id": "f5f1333211350afdd076669c3782e739ca851f5d",
          "message": "Merge pull request #206 from blooop/wayfinder/devlaunch-168\n\nOne cold-path setup pass: the hostname folds into the probe's trip, with per-stage outcomes",
          "timestamp": "2026-08-14T23:45:07+02:00",
          "tree_id": "ec28322c21c512815fdbdda4dd45aed5085ba95c",
          "url": "https://github.com/blooop/devlaunch/commit/f5f1333211350afdd076669c3782e739ca851f5d"
        },
        "date": 1786744018623,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.156413,
            "range": "± 0.066612",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.275193,
            "range": "± 0.010525",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.00074,
            "range": "± 0.000105",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.4227,
            "range": "± 0.070893",
            "unit": "s",
            "extra": "runs=5/5 wall=1.546044s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.138134,
            "range": "± 0.088655",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.352756,
            "range": "± 0.443185",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.188916,
            "range": "± 0.0116",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.97582,
            "range": "± 0.074469",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.713978,
            "range": "± 0.390428",
            "unit": "s",
            "extra": "runs=5/5 wall=9.848863s v0.26.1, Linux-X64"
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
          "id": "f5f1333211350afdd076669c3782e739ca851f5d",
          "message": "Merge pull request #206 from blooop/wayfinder/devlaunch-168\n\nOne cold-path setup pass: the hostname folds into the probe's trip, with per-stage outcomes",
          "timestamp": "2026-08-14T21:45:07Z",
          "url": "https://github.com/blooop/devlaunch/commit/f5f1333211350afdd076669c3782e739ca851f5d"
        },
        "date": 1786744160701,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.214669,
            "range": "± 0.117559",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.315893,
            "range": "± 0.122456",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000517,
            "range": "± 0.000204",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.525744,
            "range": "± 0.23287",
            "unit": "s",
            "extra": "runs=5/5 wall=1.6298s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.204324,
            "range": "± 0.039394",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.055661,
            "range": "± 0.390366",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.360273,
            "range": "± 0.047576",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.213818,
            "range": "± 0.173148",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 11.049758,
            "range": "± 0.399554",
            "unit": "s",
            "extra": "runs=5/5 wall=11.17111s v0.26.1, Linux-X64"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "email": "blooop@gmail.com",
            "name": "Austin Gregg-Smith",
            "username": "blooop"
          },
          "committer": {
            "email": "noreply@github.com",
            "name": "GitHub",
            "username": "web-flow"
          },
          "distinct": true,
          "id": "fc800ae40ac7dbecd4fb590bc99faa140a065ada",
          "message": "Merge pull request #207 from blooop/wayfinder/devlaunch-160-dl\n\npurge, prune: name the Docker disk neither command frees",
          "timestamp": "2026-08-15T00:20:59+02:00",
          "tree_id": "a23bfa4443313dfc355d180c7dd2c2c3c759da6b",
          "url": "https://github.com/blooop/devlaunch/commit/fc800ae40ac7dbecd4fb590bc99faa140a065ada"
        },
        "date": 1786746178686,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.185999,
            "range": "± 0.187955",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.289685,
            "range": "± 0.024388",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000798,
            "range": "± 0.000103",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.52527,
            "range": "± 0.191872",
            "unit": "s",
            "extra": "runs=5/5 wall=1.656921s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.162031,
            "range": "± 0.075121",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.387818,
            "range": "± 0.076951",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.177991,
            "range": "± 0.007762",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.304796,
            "range": "± 0.83954",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.091737,
            "range": "± 0.846333",
            "unit": "s",
            "extra": "runs=5/5 wall=10.240551s v0.26.1, Linux-X64"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "email": "blooop@gmail.com",
            "name": "Austin Gregg-Smith",
            "username": "blooop"
          },
          "committer": {
            "email": "noreply@github.com",
            "name": "GitHub",
            "username": "web-flow"
          },
          "distinct": true,
          "id": "ba1441047f0caf39cd88925af84386353b070c46",
          "message": "Merge pull request #208 from blooop/wayfinder/devlaunch-162\n\nPin the bare-to-clone object hardlink with a test and state it as the design",
          "timestamp": "2026-08-15T00:40:51+02:00",
          "tree_id": "00ff479a5e071892c322d6091d63d22c4166fc22",
          "url": "https://github.com/blooop/devlaunch/commit/ba1441047f0caf39cd88925af84386353b070c46"
        },
        "date": 1786747366830,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.250994,
            "range": "± 0.109361",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.308688,
            "range": "± 0.15684",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000665,
            "range": "± 0.000099",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.606078,
            "range": "± 0.164912",
            "unit": "s",
            "extra": "runs=5/5 wall=1.730303s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.167056,
            "range": "± 0.069784",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.540599,
            "range": "± 0.042458",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.2736,
            "range": "± 0.009329",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.149141,
            "range": "± 0.104246",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.109049,
            "range": "± 0.142317",
            "unit": "s",
            "extra": "runs=5/5 wall=10.23611s v0.26.1, Linux-X64"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "email": "blooop@gmail.com",
            "name": "Austin Gregg-Smith",
            "username": "blooop"
          },
          "committer": {
            "email": "noreply@github.com",
            "name": "GitHub",
            "username": "web-flow"
          },
          "distinct": true,
          "id": "e3d4639edc53328f592fbdb1d78721b59c4536a5",
          "message": "Merge pull request #209 from blooop/wayfinder/devlaunch-163\n\nThe bare cache becomes the repo's LFS store; workspaces hardlink from it",
          "timestamp": "2026-08-15T01:14:43+02:00",
          "tree_id": "0c281d7eb896ec7fcd9327caa4afd74456a05078",
          "url": "https://github.com/blooop/devlaunch/commit/e3d4639edc53328f592fbdb1d78721b59c4536a5"
        },
        "date": 1786749399284,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.131587,
            "range": "± 0.103081",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.275699,
            "range": "± 1.03272",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000678,
            "range": "± 0.000048",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.475736,
            "range": "± 1.024296",
            "unit": "s",
            "extra": "runs=5/5 wall=1.599436s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.124847,
            "range": "± 0.041789",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.281073,
            "range": "± 0.904872",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.168698,
            "range": "± 0.007212",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.911424,
            "range": "± 0.078596",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.667042,
            "range": "± 0.841094",
            "unit": "s",
            "extra": "runs=5/5 wall=9.785851s v0.26.1, Linux-X64"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "email": "blooop@gmail.com",
            "name": "Austin Gregg-Smith",
            "username": "blooop"
          },
          "committer": {
            "email": "noreply@github.com",
            "name": "GitHub",
            "username": "web-flow"
          },
          "distinct": true,
          "id": "9013d20523275cbb3d114d6160b53a8682302802",
          "message": "Merge pull request #210 from blooop/wayfinder/devlaunch-88\n\nFollow the recorded devpod workspace id, and reconcile the orphans",
          "timestamp": "2026-08-15T02:23:20+02:00",
          "tree_id": "99fabef8263db65019f4979ef60f059cd779f953",
          "url": "https://github.com/blooop/devlaunch/commit/9013d20523275cbb3d114d6160b53a8682302802"
        },
        "date": 1786753515852,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.175963,
            "range": "± 0.11693",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.259193,
            "range": "± 0.037711",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000821,
            "range": "± 0.00009",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.47017,
            "range": "± 0.118802",
            "unit": "s",
            "extra": "runs=5/5 wall=1.598543s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.199714,
            "range": "± 0.118773",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.298813,
            "range": "± 0.565284",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.169651,
            "range": "± 0.008125",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.944969,
            "range": "± 0.146224",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.690137,
            "range": "± 0.591844",
            "unit": "s",
            "extra": "runs=5/5 wall=9.829752s v0.26.1, Linux-X64"
          }
        ]
      }
    ]
  }
}