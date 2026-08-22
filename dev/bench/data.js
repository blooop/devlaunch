window.BENCHMARK_DATA = {
  "lastUpdate": 1787431428091,
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
          "id": "795e393eee7dbd80eed4692d22f82b382b85a912",
          "message": "Merge pull request #211 from blooop/wayfinder/devlaunch-187\n\nPin git's message locale at the last site that classified translated stderr",
          "timestamp": "2026-08-15T02:46:18+02:00",
          "tree_id": "fa19c0acdf0aeda328e8d3e9bd5b77c5450a2eda",
          "url": "https://github.com/blooop/devlaunch/commit/795e393eee7dbd80eed4692d22f82b382b85a912"
        },
        "date": 1786754904041,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.301431,
            "range": "± 0.079223",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.323403,
            "range": "± 0.006834",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000622,
            "range": "± 0.00011",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.639413,
            "range": "± 0.078764",
            "unit": "s",
            "extra": "runs=5/5 wall=1.759588s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.291841,
            "range": "± 0.121039",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.876656,
            "range": "± 0.521014",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.325483,
            "range": "± 0.036109",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.789295,
            "range": "± 0.280489",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 11.76502,
            "range": "± 0.679322",
            "unit": "s",
            "extra": "runs=5/5 wall=11.885547s v0.26.1, Linux-X64"
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
          "id": "a68aab81bff6c05eaa173de73e0fde6caa9693ae",
          "message": "Merge pull request #213 from blooop/wayfinder/devlaunch-191\n\nResolve the login profile one way, in every writer",
          "timestamp": "2026-08-15T03:10:55+02:00",
          "tree_id": "200099ce42b682c3960fc278b36c5d91dd7cbc2f",
          "url": "https://github.com/blooop/devlaunch/commit/a68aab81bff6c05eaa173de73e0fde6caa9693ae"
        },
        "date": 1786756375555,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 0.991692,
            "range": "± 0.129659",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.229294,
            "range": "± 0.01926",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000445,
            "range": "± 0.000061",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.241335,
            "range": "± 0.136364",
            "unit": "s",
            "extra": "runs=5/5 wall=1.342155s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 0.912765,
            "range": "± 0.042207",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.497044,
            "range": "± 0.376705",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.289927,
            "range": "± 0.09825",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.083909,
            "range": "± 0.098648",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.895495,
            "range": "± 0.401152",
            "unit": "s",
            "extra": "runs=5/5 wall=8.999245s v0.26.1, Linux-X64"
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
          "id": "05dd73da676454c5f35c4a8f444b366591d290e1",
          "message": "Merge pull request #214 from blooop/wayfinder/devlaunch-192\n\nPoint the executable-doc guard at the README as well",
          "timestamp": "2026-08-15T03:39:11+02:00",
          "tree_id": "af0e61afb540b3e41a5a4a3495bbd799f7d8d3ec",
          "url": "https://github.com/blooop/devlaunch/commit/05dd73da676454c5f35c4a8f444b366591d290e1"
        },
        "date": 1786758063921,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.140165,
            "range": "± 0.067689",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.280749,
            "range": "± 0.012071",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000798,
            "range": "± 0.000107",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.422456,
            "range": "± 0.073856",
            "unit": "s",
            "extra": "runs=5/5 wall=1.550219s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.135071,
            "range": "± 0.11122",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.285717,
            "range": "± 0.54899",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.161787,
            "range": "± 0.025266",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.257412,
            "range": "± 0.112515",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.975673,
            "range": "± 0.561607",
            "unit": "s",
            "extra": "runs=5/5 wall=10.108419s v0.26.1, Linux-X64"
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
          "id": "0e392ba6f8caa74a618fbe88b930546848481d52",
          "message": "Merge pull request #215 from blooop/wayfinder/devlaunch-182\n\nSay which of three things a purge removed, not two",
          "timestamp": "2026-08-15T04:05:31+02:00",
          "tree_id": "6d14c784acd898fde42927097e07114097dbfc82",
          "url": "https://github.com/blooop/devlaunch/commit/0e392ba6f8caa74a618fbe88b930546848481d52"
        },
        "date": 1786759656580,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.12933,
            "range": "± 0.122174",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.296709,
            "range": "± 0.137278",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000865,
            "range": "± 0.000115",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.506752,
            "range": "± 0.113077",
            "unit": "s",
            "extra": "runs=5/5 wall=1.641354s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.112784,
            "range": "± 0.089775",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.60138,
            "range": "± 0.082041",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.257245,
            "range": "± 0.004263",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.112354,
            "range": "± 0.178964",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.154731,
            "range": "± 0.293548",
            "unit": "s",
            "extra": "runs=5/5 wall=10.28693s v0.26.1, Linux-X64"
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
          "id": "630f5d36f460fe144019b5634bf8b87f53afab40",
          "message": "Merge pull request #216 from blooop/wayfinder/devlaunch-184\n\nMake the prune spawn-count fixture earn its removal from real git",
          "timestamp": "2026-08-15T04:59:39+02:00",
          "tree_id": "1281654aa764ad982bc1a84bb19a1f25343a657d",
          "url": "https://github.com/blooop/devlaunch/commit/630f5d36f460fe144019b5634bf8b87f53afab40"
        },
        "date": 1786762887992,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.101093,
            "range": "± 0.0524",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.259803,
            "range": "± 0.020213",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000838,
            "range": "± 0.000114",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.379187,
            "range": "± 0.041671",
            "unit": "s",
            "extra": "runs=5/5 wall=1.501027s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.155344,
            "range": "± 0.171651",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.311879,
            "range": "± 0.084955",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.168077,
            "range": "± 0.008145",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.141407,
            "range": "± 0.187256",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.800674,
            "range": "± 0.294523",
            "unit": "s",
            "extra": "runs=5/5 wall=9.923866s v0.26.1, Linux-X64"
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
          "id": "4e96e53649216d086362ebd267fa44bd4e4ab742",
          "message": "Merge pull request #218 from blooop/wayfinder/devlaunch-188\n\nRetire the auto_fetch knob, and let stale configs keep it harmlessly",
          "timestamp": "2026-08-15T05:16:58+02:00",
          "tree_id": "20de6be5768e93c3755583ed7238a25e2f9edf91",
          "url": "https://github.com/blooop/devlaunch/commit/4e96e53649216d086362ebd267fa44bd4e4ab742"
        },
        "date": 1786763948568,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.026162,
            "range": "± 0.093926",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.221523,
            "range": "± 0.003879",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.00061,
            "range": "± 0.000121",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.243095,
            "range": "± 0.093244",
            "unit": "s",
            "extra": "runs=5/5 wall=1.36589s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 0.918389,
            "range": "± 0.071626",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.483641,
            "range": "± 0.061979",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.232238,
            "range": "± 0.006858",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.369436,
            "range": "± 0.047872",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.912051,
            "range": "± 0.09797",
            "unit": "s",
            "extra": "runs=5/5 wall=9.03679s v0.26.1, Linux-X64"
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
          "id": "9681c6be2db6382dbb38578e6603c891a0e34f42",
          "message": "Merge pull request #219 from blooop/wayfinder/devlaunch-108\n\nMake the claude-code feature's read-only mounts real",
          "timestamp": "2026-08-15T06:06:27+02:00",
          "tree_id": "5da48fa0d35408400cd55c53a7945cdca5c23e2e",
          "url": "https://github.com/blooop/devlaunch/commit/9681c6be2db6382dbb38578e6603c891a0e34f42"
        },
        "date": 1786766933786,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.14178,
            "range": "± 0.060599",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.294224,
            "range": "± 0.036372",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000511,
            "range": "± 0.00002",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.474463,
            "range": "± 0.051975",
            "unit": "s",
            "extra": "runs=5/5 wall=1.594641s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.209075,
            "range": "± 0.095574",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.935836,
            "range": "± 0.093308",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.369602,
            "range": "± 0.012068",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.560007,
            "range": "± 0.206573",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 11.212571,
            "range": "± 0.301437",
            "unit": "s",
            "extra": "runs=5/5 wall=11.335037s v0.26.1, Linux-X64"
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
          "id": "82a60fee8a20ee17201a74c804e572e79041a2a8",
          "message": "Merge pull request #220 from blooop/wayfinder/devlaunch-183\n\nOpt-in dotfiles refresh on attach",
          "timestamp": "2026-08-15T06:36:25+02:00",
          "tree_id": "4e916338b7ee2cb70193e380ae967c63c45db997",
          "url": "https://github.com/blooop/devlaunch/commit/82a60fee8a20ee17201a74c804e572e79041a2a8"
        },
        "date": 1786768713116,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.232068,
            "range": "± 0.057876",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.327069,
            "range": "± 0.006119",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000736,
            "range": "± 0.000101",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.567477,
            "range": "± 0.062212",
            "unit": "s",
            "extra": "runs=5/5 wall=1.690314s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.298282,
            "range": "± 0.098959",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.639875,
            "range": "± 0.45076",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.347558,
            "range": "± 0.025599",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.724053,
            "range": "± 0.200525",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 11.294664,
            "range": "± 0.451609",
            "unit": "s",
            "extra": "runs=5/5 wall=11.420426s v0.26.1, Linux-X64"
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
          "id": "67e2acf821d1ed51612fdb31e403b9a2aa63f372",
          "message": "Merge pull request #222 from blooop/wayfinder/devlaunch-221\n\nRider sweep: the small debts this week's reviews parked",
          "timestamp": "2026-08-15T07:35:32+02:00",
          "tree_id": "756fb67bf5675bfe939a13a9044687c9c72a67e7",
          "url": "https://github.com/blooop/devlaunch/commit/67e2acf821d1ed51612fdb31e403b9a2aa63f372"
        },
        "date": 1786772258778,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.219963,
            "range": "± 0.136679",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.288599,
            "range": "± 0.032964",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.00075,
            "range": "± 0.000089",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.509327,
            "range": "± 0.147334",
            "unit": "s",
            "extra": "runs=5/5 wall=1.644777s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.276455,
            "range": "± 0.164805",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.774564,
            "range": "± 0.079203",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.335263,
            "range": "± 0.021097",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.589707,
            "range": "± 0.192653",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.860513,
            "range": "± 0.398242",
            "unit": "s",
            "extra": "runs=5/5 wall=10.995629s v0.26.1, Linux-X64"
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
          "id": "fd4c50f335f49d7d5323ce89a82014bd647100e2",
          "message": "Merge pull request #223 from blooop/wayfinder/devlaunch-212\n\nLayer GIT_SSH_COMMAND on the inherited env when pushing a branch",
          "timestamp": "2026-08-15T10:24:31+02:00",
          "tree_id": "c367ad7ffd46b8d2949baceb2158e2954e11c416",
          "url": "https://github.com/blooop/devlaunch/commit/fd4c50f335f49d7d5323ce89a82014bd647100e2"
        },
        "date": 1786782399987,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.315862,
            "range": "± 0.044363",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.330897,
            "range": "± 0.006231",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000698,
            "range": "± 0.000103",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.645209,
            "range": "± 0.044263",
            "unit": "s",
            "extra": "runs=5/5 wall=1.774479s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.330497,
            "range": "± 0.070159",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.444201,
            "range": "± 0.050614",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.328803,
            "range": "± 0.011775",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.805611,
            "range": "± 0.134737",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.897479,
            "range": "± 0.148179",
            "unit": "s",
            "extra": "runs=5/5 wall=11.023219s v0.26.1, Linux-X64"
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
          "id": "1c93b3bd7a4b0e3ee421be7837acba2fe53074d8",
          "message": "Merge pull request #226 from blooop/wayfinder/devlaunch-217\n\nResolve the provider runner at call time, not at import",
          "timestamp": "2026-08-15T10:47:25+02:00",
          "tree_id": "2b37673d8cb8eaf0a34c1b80806663ed3522c1f6",
          "url": "https://github.com/blooop/devlaunch/commit/1c93b3bd7a4b0e3ee421be7837acba2fe53074d8"
        },
        "date": 1786783761150,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.104158,
            "range": "± 0.070982",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.260292,
            "range": "± 0.00906",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000681,
            "range": "± 0.000155",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.379496,
            "range": "± 0.06553",
            "unit": "s",
            "extra": "runs=5/5 wall=1.514035s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.089908,
            "range": "± 0.1788",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.668674,
            "range": "± 0.083125",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.261919,
            "range": "± 0.00688",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.018249,
            "range": "± 0.098715",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.21883,
            "range": "± 0.230875",
            "unit": "s",
            "extra": "runs=5/5 wall=10.366015s v0.26.1, Linux-X64"
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
          "id": "044498b72100c95cb9c060fcecb056599eddb0f7",
          "message": "Merge pull request #228 from blooop/wayfinder/devlaunch-180\n\nLeave the schema header behind when a rename was refused",
          "timestamp": "2026-08-15T12:49:01+02:00",
          "tree_id": "607d372ec339e097673d4df7614db68f7598cbc4",
          "url": "https://github.com/blooop/devlaunch/commit/044498b72100c95cb9c060fcecb056599eddb0f7"
        },
        "date": 1786791076658,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.19689,
            "range": "± 0.194665",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.460454,
            "range": "± 0.107461",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000506,
            "range": "± 0.000065",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.577441,
            "range": "± 0.242642",
            "unit": "s",
            "extra": "runs=5/5 wall=1.676542s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.225124,
            "range": "± 0.145535",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.659813,
            "range": "± 0.075093",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.31316,
            "range": "± 0.018287",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.926613,
            "range": "± 0.27713",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.129958,
            "range": "± 0.421944",
            "unit": "s",
            "extra": "runs=5/5 wall=10.228652s v0.26.1, Linux-X64"
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
          "id": "1bb34c4a9446a7837f6097273bf071867d7f00ae",
          "message": "Merge pull request #229 from blooop/wayfinder/devlaunch-227\n\nOffer reconcile before delete in the orphaned-containers notice",
          "timestamp": "2026-08-15T13:06:50+02:00",
          "tree_id": "b8e1ad474716987d4e40025de578ca8742aeaa9f",
          "url": "https://github.com/blooop/devlaunch/commit/1bb34c4a9446a7837f6097273bf071867d7f00ae"
        },
        "date": 1786792119650,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.09797,
            "range": "± 0.081254",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.258225,
            "range": "± 0.03915",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.00084,
            "range": "± 0.000125",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.415219,
            "range": "± 0.084622",
            "unit": "s",
            "extra": "runs=5/5 wall=1.539795s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.066559,
            "range": "± 0.055241",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.396666,
            "range": "± 0.511471",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.16364,
            "range": "± 0.006514",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.06212,
            "range": "± 0.176504",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.700998,
            "range": "± 0.620696",
            "unit": "s",
            "extra": "runs=5/5 wall=9.82267s v0.26.1, Linux-X64"
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
          "id": "1c686af276667e2059787ee0b4ad8610d641909b",
          "message": "Merge pull request #231 from blooop/wayfinder/devlaunch-224\n\nRead a URL-shaped workspace source as a remote, not as a path",
          "timestamp": "2026-08-15T13:37:46+02:00",
          "tree_id": "26f13a03e0ddb353e1dc62f51311624d65995e1c",
          "url": "https://github.com/blooop/devlaunch/commit/1c686af276667e2059787ee0b4ad8610d641909b"
        },
        "date": 1786793978445,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.099545,
            "range": "± 0.084075",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.281405,
            "range": "± 0.013112",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000612,
            "range": "± 0.000107",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.361441,
            "range": "± 0.081107",
            "unit": "s",
            "extra": "runs=5/5 wall=1.511254s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.111205,
            "range": "± 0.077019",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.266347,
            "range": "± 0.411459",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.166166,
            "range": "± 0.00748",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.180367,
            "range": "± 1.27708",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.698213,
            "range": "± 1.302373",
            "unit": "s",
            "extra": "runs=5/5 wall=9.820177s v0.26.1, Linux-X64"
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
          "id": "14a464e12319a240ab1c4e3943a54b55afdfc607",
          "message": "Merge pull request #233 from blooop/wayfinder/devlaunch-225\n\nQuote the ssh key path into GIT_SSH_COMMAND, and name a silent push failure",
          "timestamp": "2026-08-15T14:09:34+02:00",
          "tree_id": "9856040f61090cb546f2c1707cb5c59432508be3",
          "url": "https://github.com/blooop/devlaunch/commit/14a464e12319a240ab1c4e3943a54b55afdfc607"
        },
        "date": 1786795905703,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.353312,
            "range": "± 0.046592",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.358948,
            "range": "± 0.082656",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000937,
            "range": "± 0.00016",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.73721,
            "range": "± 0.094257",
            "unit": "s",
            "extra": "runs=5/5 wall=1.872694s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.31576,
            "range": "± 0.081118",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.546617,
            "range": "± 0.555333",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.314882,
            "range": "± 0.036765",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 6.915513,
            "range": "± 0.052778",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 12.097802,
            "range": "± 0.526295",
            "unit": "s",
            "extra": "runs=5/5 wall=12.234708s v0.26.1, Linux-X64"
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
          "id": "05500fbeda6a415167bb056120dec509aa350eb9",
          "message": "Merge pull request #235 from blooop/wayfinder/devlaunch-230\n\nPin the repair the migration's notice promises, at the seam that owes it",
          "timestamp": "2026-08-15T14:36:07+02:00",
          "tree_id": "bd3c2bfe0523e9c327a6e02be6215ab996fe7de2",
          "url": "https://github.com/blooop/devlaunch/commit/05500fbeda6a415167bb056120dec509aa350eb9"
        },
        "date": 1786797474615,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.188024,
            "range": "± 0.125375",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.221243,
            "range": "± 0.005049",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000629,
            "range": "± 0.0001",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.410104,
            "range": "± 0.128754",
            "unit": "s",
            "extra": "runs=5/5 wall=1.539638s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 0.932962,
            "range": "± 0.076234",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.182016,
            "range": "± 0.055741",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.230473,
            "range": "± 0.00898",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.767725,
            "range": "± 0.054204",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.208307,
            "range": "± 0.11095",
            "unit": "s",
            "extra": "runs=5/5 wall=9.337609s v0.26.1, Linux-X64"
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
          "id": "e77f25f366bad03bb95cfd301e118ce0841a533b",
          "message": "Merge pull request #278 from blooop/fix/bench-shared-pixi-cache-uid\n\nWiden the shared pixi cache for the container, so the bench can launch at all",
          "timestamp": "2026-08-20T18:51:52+01:00",
          "tree_id": "3bae3e669b664fdaea57586b1489b733fc64e850",
          "url": "https://github.com/blooop/devlaunch/commit/e77f25f366bad03bb95cfd301e118ce0841a533b"
        },
        "date": 1787248431693,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.247231,
            "range": "± 0.11465",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.325319,
            "range": "± 0.015994",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000602,
            "range": "± 0.00012",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.571797,
            "range": "± 0.112156",
            "unit": "s",
            "extra": "runs=5/5 wall=1.696735s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.298823,
            "range": "± 0.040942",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.294493,
            "range": "± 0.823938",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.339107,
            "range": "± 0.013364",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.24556,
            "range": "± 0.039372",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.04182,
            "range": "± 0.812851",
            "unit": "s",
            "extra": "runs=5/5 wall=10.171276s v0.26.1, Linux-X64"
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
          "id": "ba98fad838e81fb16bff891c2baa0cbea6566a71",
          "message": "Merge pull request #280 from blooop/feat/prebuild-arm64-leg\n\nPublish the devcontainer prebuild for arm64 as well as amd64",
          "timestamp": "2026-08-20T18:52:01+01:00",
          "tree_id": "178fd4899b101c16089d670da1c4b2ef073d0ea4",
          "url": "https://github.com/blooop/devlaunch/commit/ba98fad838e81fb16bff891c2baa0cbea6566a71"
        },
        "date": 1787248543261,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.208955,
            "range": "± 0.130577",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.277971,
            "range": "± 0.006953",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000699,
            "range": "± 0.000052",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.490727,
            "range": "± 0.133296",
            "unit": "s",
            "extra": "runs=5/5 wall=1.616928s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.215735,
            "range": "± 0.093325",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.283106,
            "range": "± 0.527559",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.209385,
            "range": "± 0.022417",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.125483,
            "range": "± 0.149624",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.852546,
            "range": "± 0.633414",
            "unit": "s",
            "extra": "runs=5/5 wall=8.981326s v0.26.1, Linux-X64"
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
          "id": "54c21f79a16d9fab9cf84657d70a1b5ecb577af9",
          "message": "Merge pull request #272 from blooop/improve_readme\n\nOrder the README for human readers first, release 0.1.3",
          "timestamp": "2026-08-20T19:13:05+01:00",
          "tree_id": "0f68502737bedcb49256db1d4f457b8975fdb080",
          "url": "https://github.com/blooop/devlaunch/commit/54c21f79a16d9fab9cf84657d70a1b5ecb577af9"
        },
        "date": 1787249716675,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.174759,
            "range": "± 0.084999",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.292305,
            "range": "± 0.000884",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000506,
            "range": "± 0.000029",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.468009,
            "range": "± 0.085783",
            "unit": "s",
            "extra": "runs=5/5 wall=1.567855s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.143218,
            "range": "± 0.094419",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.657755,
            "range": "± 0.961168",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.51939,
            "range": "± 0.124408",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.690302,
            "range": "± 0.184543",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.987296,
            "range": "± 1.135719",
            "unit": "s",
            "extra": "runs=5/5 wall=9.093683s v0.26.1, Linux-X64"
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
          "id": "22291bd896441dd629d33008c3d436c7a6fd949d",
          "message": "Merge pull request #279 from blooop/docs/prebuild-hash-covers-the-recipe\n\nSay what the prebuild tag promises, instead of pinning claude-shim",
          "timestamp": "2026-08-20T19:17:19+01:00",
          "tree_id": "4268ed4a3cdcb3f729bc38fbda9dbb5a7f897e40",
          "url": "https://github.com/blooop/devlaunch/commit/22291bd896441dd629d33008c3d436c7a6fd949d"
        },
        "date": 1787249942713,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.009104,
            "range": "± 0.137192",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.234845,
            "range": "± 0.018127",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000724,
            "range": "± 0.000081",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.270165,
            "range": "± 0.140137",
            "unit": "s",
            "extra": "runs=5/5 wall=1.397795s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.067669,
            "range": "± 0.103616",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.072161,
            "range": "± 0.419065",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.236635,
            "range": "± 0.013415",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.887437,
            "range": "± 0.163915",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.314936,
            "range": "± 0.453837",
            "unit": "s",
            "extra": "runs=5/5 wall=8.449601s v0.26.1, Linux-X64"
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
          "id": "29f12b990fa54a95bb4a2b8b28bb1aba399d5f22",
          "message": "Merge pull request #281 from blooop/fix/devcontainer-pixi-reads-the-committed-lock\n\nPin the devcontainer's pixi to one that can read the committed lock",
          "timestamp": "2026-08-20T19:28:17+01:00",
          "tree_id": "0972e953a30078cf95f1975f5d547015cb281785",
          "url": "https://github.com/blooop/devlaunch/commit/29f12b990fa54a95bb4a2b8b28bb1aba399d5f22"
        },
        "date": 1787250623754,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.344592,
            "range": "± 0.026957",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.339421,
            "range": "± 0.009243",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000781,
            "range": "± 0.000109",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.68967,
            "range": "± 0.035277",
            "unit": "s",
            "extra": "runs=5/5 wall=1.818772s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.394976,
            "range": "± 0.492613",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.409311,
            "range": "± 0.598203",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.32023,
            "range": "± 0.143588",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.345452,
            "range": "± 0.222858",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.288833,
            "range": "± 0.563952",
            "unit": "s",
            "extra": "runs=5/5 wall=10.420328s v0.26.1, Linux-X64"
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
          "id": "1b01ac2c138b03c7462a2d95b4a362624466fb2a",
          "message": "Merge pull request #277 from blooop/fix/tools-own-pixi-home\n\nInstall devlaunch's tools into a pixi home of its own",
          "timestamp": "2026-08-20T19:31:02+01:00",
          "tree_id": "3c67d696c184c2634f30c22066267384d6b6d5b1",
          "url": "https://github.com/blooop/devlaunch/commit/1b01ac2c138b03c7462a2d95b4a362624466fb2a"
        },
        "date": 1787250784153,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.403247,
            "range": "± 0.08176",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.33978,
            "range": "± 0.008904",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000692,
            "range": "± 0.000109",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.73655,
            "range": "± 0.087834",
            "unit": "s",
            "extra": "runs=5/5 wall=1.869381s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.277959,
            "range": "± 0.080368",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.590535,
            "range": "± 0.411546",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.337623,
            "range": "± 0.035383",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.366163,
            "range": "± 0.123632",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.581565,
            "range": "± 0.498779",
            "unit": "s",
            "extra": "runs=5/5 wall=9.719202s v0.26.1, Linux-X64"
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
          "id": "9b16d90d85d839cb49854b992f8f59ae8b80d0ad",
          "message": "Merge pull request #284 from blooop/feat/rust-in-pixi-env\n\nPut the Rust toolchain in the pixi env",
          "timestamp": "2026-08-20T19:39:30+01:00",
          "tree_id": "70d9fc37bea6aef359b446e82cdc769888964ea5",
          "url": "https://github.com/blooop/devlaunch/commit/9b16d90d85d839cb49854b992f8f59ae8b80d0ad"
        },
        "date": 1787251291975,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.043892,
            "range": "± 0.02913",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.231101,
            "range": "± 0.012373",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000496,
            "range": "± 0.00006",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.266053,
            "range": "± 0.035421",
            "unit": "s",
            "extra": "runs=5/5 wall=1.366647s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.019925,
            "range": "± 0.04068",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.67047,
            "range": "± 0.209234",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.230359,
            "range": "± 0.109143",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.262966,
            "range": "± 0.28248",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.165467,
            "range": "± 0.37425",
            "unit": "s",
            "extra": "runs=5/5 wall=8.269278s v0.26.1, Linux-X64"
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
          "id": "3ac703882795862334fd614f6165681944a90918",
          "message": "Merge pull request #283 from blooop/longer_container_name\n\nWiden the workspace-id budget to 47, one inside devpod's fatal 48",
          "timestamp": "2026-08-20T19:42:22+01:00",
          "tree_id": "0edea619ed6807d9a054e139b0ade5e262d53845",
          "url": "https://github.com/blooop/devlaunch/commit/3ac703882795862334fd614f6165681944a90918"
        },
        "date": 1787251461630,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.234619,
            "range": "± 0.042298",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.269868,
            "range": "± 0.010741",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.00065,
            "range": "± 0.000065",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.505972,
            "range": "± 0.043097",
            "unit": "s",
            "extra": "runs=5/5 wall=1.636654s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.091668,
            "range": "± 0.148948",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.321028,
            "range": "± 0.113127",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.369914,
            "range": "± 0.102951",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.233706,
            "range": "± 0.056336",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.196849,
            "range": "± 0.252837",
            "unit": "s",
            "extra": "runs=5/5 wall=9.325912s v0.26.1, Linux-X64"
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
          "id": "f70a934694d5cd9e8ded80717bf9e8c9389020b6",
          "message": "Merge pull request #285 from blooop/release/0.3.0\n\nRelease 0.3.0",
          "timestamp": "2026-08-20T19:47:52+01:00",
          "tree_id": "68a6ea0254fe4dae6fdca00547a02a3f3ab475e3",
          "url": "https://github.com/blooop/devlaunch/commit/f70a934694d5cd9e8ded80717bf9e8c9389020b6"
        },
        "date": 1787251820872,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.207427,
            "range": "± 0.056479",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.29969,
            "range": "± 0.009161",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000501,
            "range": "± 0.000095",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.508254,
            "range": "± 0.06494",
            "unit": "s",
            "extra": "runs=5/5 wall=1.61702s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.191345,
            "range": "± 0.053771",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.705327,
            "range": "± 0.103714",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.534012,
            "range": "± 0.471781",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.69513,
            "range": "± 0.180031",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.276017,
            "range": "± 0.529505",
            "unit": "s",
            "extra": "runs=5/5 wall=9.3824s v0.26.1, Linux-X64"
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
          "id": "1566c86189fd7ccdb3cc2408463e9ac4d3b77817",
          "message": "Merge pull request #286 from blooop/feat/dev-loop-rust-build\n\nDev loop: point dl-next and pixi run dl at the Rust build",
          "timestamp": "2026-08-20T20:16:27+01:00",
          "tree_id": "f700b2f5100c3e5fccbaeb8774667ba6e93dc1b7",
          "url": "https://github.com/blooop/devlaunch/commit/1566c86189fd7ccdb3cc2408463e9ac4d3b77817"
        },
        "date": 1787253535742,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.124697,
            "range": "± 0.167427",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.241049,
            "range": "± 0.009378",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.00068,
            "range": "± 0.000137",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.367167,
            "range": "± 0.163424",
            "unit": "s",
            "extra": "runs=5/5 wall=1.472697s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.022576,
            "range": "± 0.092618",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.467301,
            "range": "± 0.396958",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.323941,
            "range": "± 0.112384",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.574547,
            "range": "± 0.142835",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.345077,
            "range": "± 0.43275",
            "unit": "s",
            "extra": "runs=5/5 wall=8.451071s v0.26.1, Linux-X64"
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
            "name": "Austin Gregg-Smith",
            "username": "blooop",
            "email": "blooop@gmail.com"
          },
          "id": "03b7de255a1ed613b91b4a11eacc5e1444527312",
          "message": "style: ruff format the helper this branch added",
          "timestamp": "2026-08-22T20:36:44Z",
          "url": "https://github.com/blooop/devlaunch/commit/03b7de255a1ed613b91b4a11eacc5e1444527312"
        },
        "date": 1787431211585,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.066013,
            "range": "± 0.073134",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.257669,
            "range": "± 0.012797",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000042,
            "range": "± 0.000011",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.324556,
            "range": "± 0.085584",
            "unit": "s",
            "extra": "runs=5/5 wall=1.326504s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.037222,
            "range": "± 0.091535",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.93904,
            "range": "± 0.845921",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.167389,
            "range": "± 0.006458",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.819403,
            "range": "± 0.082416",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.954458,
            "range": "± 0.935448",
            "unit": "s",
            "extra": "runs=5/5 wall=9.956495s v0.26.1, Linux-X64"
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
          "id": "3ae4d49cb276c125d9589a6e7b1b13a642e4bab5",
          "message": "Merge pull request #373 from blooop/fix/bench-devpod-on-path\n\nPut devpod on the PATH the bench runs `dl` from",
          "timestamp": "2026-08-22T21:40:52+01:00",
          "tree_id": "a9cc8ffec18be46797083b2e76a3080db6c74275",
          "url": "https://github.com/blooop/devlaunch/commit/3ae4d49cb276c125d9589a6e7b1b13a642e4bab5"
        },
        "date": 1787431427562,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.225155,
            "range": "± 0.056097",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.273206,
            "range": "± 0.053553",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000049,
            "range": "± 0.000004",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.493477,
            "range": "± 0.079493",
            "unit": "s",
            "extra": "runs=5/5 wall=1.495591s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.212914,
            "range": "± 1.299796",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.132643,
            "range": "± 0.429956",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.191582,
            "range": "± 0.002743",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.267902,
            "range": "± 0.113942",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.929171,
            "range": "± 1.545492",
            "unit": "s",
            "extra": "runs=5/5 wall=9.931434s v0.26.1, Linux-X64"
          }
        ]
      }
    ]
  }
}