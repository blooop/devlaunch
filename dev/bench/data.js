window.BENCHMARK_DATA = {
  "lastUpdate": 1788033135554,
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
          "id": "7ec950b5f2a2317ec8019e6c9e3483ab05beffd7",
          "message": "Merge pull request #374 from blooop/hostname\n\nDrop the identity suffix from the container hostname",
          "timestamp": "2026-08-23T16:16:16+01:00",
          "tree_id": "754acdbd10ec138e2dbb32f328b86bb4ea595903",
          "url": "https://github.com/blooop/devlaunch/commit/7ec950b5f2a2317ec8019e6c9e3483ab05beffd7"
        },
        "date": 1787498347761,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.048359,
            "range": "± 0.119648",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.25137,
            "range": "± 0.017982",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000037,
            "range": "± 0.000003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.332451,
            "range": "± 0.126863",
            "unit": "s",
            "extra": "runs=5/5 wall=1.334329s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.038027,
            "range": "± 0.068524",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.876367,
            "range": "± 0.608995",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.167872,
            "range": "± 0.033092",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.283974,
            "range": "± 0.078949",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.379387,
            "range": "± 0.709071",
            "unit": "s",
            "extra": "runs=5/5 wall=8.381331s v0.26.1, Linux-X64"
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
          "id": "5eba6a7fba85548d9cc27242736f778a5dc556b0",
          "message": "Merge pull request #375 from blooop/release/0.11.0\n\nRelease 0.11.0",
          "timestamp": "2026-08-23T16:39:54+01:00",
          "tree_id": "1514a882cca697dcfd6ca55936113c89181fe8be",
          "url": "https://github.com/blooop/devlaunch/commit/5eba6a7fba85548d9cc27242736f778a5dc556b0"
        },
        "date": 1787499760757,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.143587,
            "range": "± 0.142233",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.262155,
            "range": "± 0.039103",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000042,
            "range": "± 0.000004",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.40408,
            "range": "± 0.170702",
            "unit": "s",
            "extra": "runs=5/5 wall=1.406314s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.079932,
            "range": "± 0.091805",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.862024,
            "range": "± 0.505405",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.164534,
            "range": "± 0.006226",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.89353,
            "range": "± 0.071819",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.024885,
            "range": "± 0.509878",
            "unit": "s",
            "extra": "runs=5/5 wall=9.026954s v0.26.1, Linux-X64"
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
          "id": "08ee96557154f623afedbca7fdd33158cfb7126b",
          "message": "Merge pull request #376 from blooop/remove-herdr\n\nRemove herdr agent state; release 0.12.0",
          "timestamp": "2026-08-23T18:09:41+01:00",
          "tree_id": "dbaaea987f3864afa27054fe8ec1a09e1570e89a",
          "url": "https://github.com/blooop/devlaunch/commit/08ee96557154f623afedbca7fdd33158cfb7126b"
        },
        "date": 1787505154622,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.119422,
            "range": "± 0.069053",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.315073,
            "range": "± 0.074504",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000042,
            "range": "± 0.000007",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.400626,
            "range": "± 0.138766",
            "unit": "s",
            "extra": "runs=5/5 wall=1.402868s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.144057,
            "range": "± 0.063738",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.937471,
            "range": "± 0.445642",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.174436,
            "range": "± 0.00499",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.876295,
            "range": "± 0.107034",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.25153,
            "range": "± 0.405334",
            "unit": "s",
            "extra": "runs=5/5 wall=9.25348s v0.26.1, Linux-X64"
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
          "id": "94cb7d9d22f518e0d773c31b73abe8f2aebd30ec",
          "message": "Merge pull request #365 from blooop/workflow_docs\n\ndocs: a Quickstart, with four demo GIFs on the front page",
          "timestamp": "2026-08-23T18:52:12+01:00",
          "tree_id": "bd1a95ecc3ef7ffeab055727a2d65e94baed14df",
          "url": "https://github.com/blooop/devlaunch/commit/94cb7d9d22f518e0d773c31b73abe8f2aebd30ec"
        },
        "date": 1787507701207,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.114427,
            "range": "± 0.089175",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.263145,
            "range": "± 0.006838",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000047,
            "range": "± 0.000003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.385375,
            "range": "± 0.088594",
            "unit": "s",
            "extra": "runs=5/5 wall=1.387513s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.080147,
            "range": "± 0.065926",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.977669,
            "range": "± 0.075849",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.17678,
            "range": "± 0.008969",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.063873,
            "range": "± 0.232645",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.373259,
            "range": "± 0.21144",
            "unit": "s",
            "extra": "runs=5/5 wall=9.376441s v0.26.1, Linux-X64"
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
          "id": "740757880a1a45e88d784f723eb1436b68ed53ee",
          "message": "Merge pull request #342 from blooop/wayfinder/devlaunch-341\n\nDrop the unused assert_cmd dev-dependency",
          "timestamp": "2026-08-23T22:37:08+01:00",
          "tree_id": "37f6e50531cc7f0d186cdbbd9eeeaffda7d8871a",
          "url": "https://github.com/blooop/devlaunch/commit/740757880a1a45e88d784f723eb1436b68ed53ee"
        },
        "date": 1787521213889,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.375568,
            "range": "± 0.182743",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.291908,
            "range": "± 0.012962",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000047,
            "range": "± 0.000003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.66097,
            "range": "± 0.184722",
            "unit": "s",
            "extra": "runs=5/5 wall=1.663162s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.301306,
            "range": "± 0.135221",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.132882,
            "range": "± 0.07382",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.336532,
            "range": "± 0.015874",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.587714,
            "range": "± 0.166075",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.314238,
            "range": "± 0.220816",
            "unit": "s",
            "extra": "runs=5/5 wall=10.316679s v0.26.1, Linux-X64"
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
          "id": "38da65019f6488547a9eca4ba1282d44c3251e31",
          "message": "Merge pull request #356 from blooop/wayfinder/devlaunch-307\n\nDelete the never-implemented worktree-backend docs, and guard README against --help",
          "timestamp": "2026-08-23T22:46:31+01:00",
          "tree_id": "7b673da39429fee1661c710b1fe8a506f768048d",
          "url": "https://github.com/blooop/devlaunch/commit/38da65019f6488547a9eca4ba1282d44c3251e31"
        },
        "date": 1787521766685,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.121231,
            "range": "± 0.107643",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.241711,
            "range": "± 0.018994",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000045,
            "range": "± 0.000013",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.377656,
            "range": "± 0.106552",
            "unit": "s",
            "extra": "runs=5/5 wall=1.380191s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.059566,
            "range": "± 0.061791",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.946321,
            "range": "± 0.7909",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.225167,
            "range": "± 0.007103",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.632006,
            "range": "± 0.050622",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.907874,
            "range": "± 0.77789",
            "unit": "s",
            "extra": "runs=5/5 wall=9.910301s v0.26.1, Linux-X64"
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
          "id": "13a69643f1c9b96d5e6a4d137a79d50894ab4cf5",
          "message": "Merge pull request #345 from blooop/wayfinder/devlaunch-315\n\nSingle-copy the JSON escaper and the shell quote",
          "timestamp": "2026-08-23T22:58:17+01:00",
          "tree_id": "2486735881983a30c9e4d7c810cd6718126a6d6c",
          "url": "https://github.com/blooop/devlaunch/commit/13a69643f1c9b96d5e6a4d137a79d50894ab4cf5"
        },
        "date": 1787522472810,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.217624,
            "range": "± 0.055126",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.282422,
            "range": "± 0.013585",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000026,
            "range": "± 0.000006",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.522043,
            "range": "± 0.061508",
            "unit": "s",
            "extra": "runs=5/5 wall=1.523875s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.19365,
            "range": "± 0.276025",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.654598,
            "range": "± 0.120222",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.314535,
            "range": "± 0.004778",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.307496,
            "range": "± 0.094845",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.609864,
            "range": "± 0.210032",
            "unit": "s",
            "extra": "runs=5/5 wall=9.612418s v0.26.1, Linux-X64"
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
          "id": "2a6ad4988acde3363c43361fa44fb8bfa76b8767",
          "message": "Merge pull request #351 from blooop/wayfinder/devlaunch-311\n\nGuard dl.bash's tables against the grammar, and complete the five missing flags",
          "timestamp": "2026-08-23T23:10:16+01:00",
          "tree_id": "be4bd903391cc80412b526cd4e2d101057b32910",
          "url": "https://github.com/blooop/devlaunch/commit/2a6ad4988acde3363c43361fa44fb8bfa76b8767"
        },
        "date": 1787523193842,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.242254,
            "range": "± 0.172307",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.302253,
            "range": "± 0.042182",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000047,
            "range": "± 0.000003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.567578,
            "range": "± 0.151345",
            "unit": "s",
            "extra": "runs=5/5 wall=1.569827s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.258446,
            "range": "± 0.136997",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.121731,
            "range": "± 0.564129",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.193253,
            "range": "± 0.010827",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.424928,
            "range": "± 0.091266",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.222576,
            "range": "± 0.559665",
            "unit": "s",
            "extra": "runs=5/5 wall=10.22498s v0.26.1, Linux-X64"
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
          "id": "a0c0ba294237ff4d9c6cafa730bde4787b503f11",
          "message": "Merge pull request #353 from blooop/wayfinder/devlaunch-310\n\nFix --ignore-not-found and drive one conformance corpus through both devpod fakes",
          "timestamp": "2026-08-23T23:20:25+01:00",
          "tree_id": "f93ed424b1cb9154aa132149d084a430d772cf61",
          "url": "https://github.com/blooop/devlaunch/commit/a0c0ba294237ff4d9c6cafa730bde4787b503f11"
        },
        "date": 1787523787547,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.144861,
            "range": "± 0.071026",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.244039,
            "range": "± 0.015448",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000038,
            "range": "± 0.000002",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.398544,
            "range": "± 0.079672",
            "unit": "s",
            "extra": "runs=5/5 wall=1.400544s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 0.889179,
            "range": "± 0.041861",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.912581,
            "range": "± 0.150211",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.163873,
            "range": "± 0.008213",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.265157,
            "range": "± 0.097258",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.218919,
            "range": "± 0.250631",
            "unit": "s",
            "extra": "runs=5/5 wall=8.220865s v0.26.1, Linux-X64"
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
          "id": "e4d701d0d53b1140fa2ad5e28b847db8dd94db17",
          "message": "Merge pull request #343 from blooop/wayfinder/devlaunch-302\n\nFix the capture and session timeout hang",
          "timestamp": "2026-08-23T23:51:41+01:00",
          "tree_id": "3a5f9ac70deaf161a1f359e6a6c9ae38a558264f",
          "url": "https://github.com/blooop/devlaunch/commit/e4d701d0d53b1140fa2ad5e28b847db8dd94db17"
        },
        "date": 1787525689213,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.423192,
            "range": "± 0.284778",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.383138,
            "range": "± 0.078281",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000055,
            "range": "± 0.000009",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.769334,
            "range": "± 0.359239",
            "unit": "s",
            "extra": "runs=5/5 wall=1.771748s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.31751,
            "range": "± 0.043142",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.242019,
            "range": "± 0.465131",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.309304,
            "range": "± 0.018229",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.430719,
            "range": "± 0.155407",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.423415,
            "range": "± 0.381379",
            "unit": "s",
            "extra": "runs=5/5 wall=10.426308s v0.26.1, Linux-X64"
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
          "id": "fb8b3407eb2db333ae956fb939fce1d29b018322",
          "message": "Merge pull request #348 from blooop/wayfinder/devlaunch-304\n\nSIGTERM and SIGHUP run the SIGINT drain",
          "timestamp": "2026-08-24T00:08:24+01:00",
          "tree_id": "ad6a3403e2d20995d47027077a70508599fbeaca",
          "url": "https://github.com/blooop/devlaunch/commit/fb8b3407eb2db333ae956fb939fce1d29b018322"
        },
        "date": 1787526680363,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.079374,
            "range": "± 0.150345",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.261282,
            "range": "± 0.021697",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000039,
            "range": "± 0.000003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.356114,
            "range": "± 0.156516",
            "unit": "s",
            "extra": "runs=5/5 wall=1.358023s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 0.982178,
            "range": "± 0.114421",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.292031,
            "range": "± 0.526884",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.189861,
            "range": "± 0.037494",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.745183,
            "range": "± 0.382795",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.362816,
            "range": "± 0.588977",
            "unit": "s",
            "extra": "runs=5/5 wall=9.364781s v0.26.1, Linux-X64"
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
          "id": "a5f7ed8aa7f27cead9a7e476004a9e1b6a4abfb5",
          "message": "Merge pull request #347 from blooop/wayfinder/devlaunch-338\n\nSplit the API snapshots: runner crate, api-vs-rest",
          "timestamp": "2026-08-24T00:49:35+01:00",
          "tree_id": "cf6fae91ce7c70ef138e292592b9d142f220b2f0",
          "url": "https://github.com/blooop/devlaunch/commit/a5f7ed8aa7f27cead9a7e476004a9e1b6a4abfb5"
        },
        "date": 1787529174518,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.141909,
            "range": "± 0.118734",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.290027,
            "range": "± 0.0207",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000032,
            "range": "± 0.000005",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.409449,
            "range": "± 0.123023",
            "unit": "s",
            "extra": "runs=5/5 wall=1.411666s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.023177,
            "range": "± 0.045519",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.265805,
            "range": "± 0.287117",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.27932,
            "range": "± 0.069137",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.79123,
            "range": "± 0.283627",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.736822,
            "range": "± 0.454984",
            "unit": "s",
            "extra": "runs=5/5 wall=9.738888s v0.26.1, Linux-X64"
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
          "id": "217f253b02ee16ef564662985f54268ef431f3fb",
          "message": "Merge pull request #380 from blooop/fix/picker-distinctness-ignores-padding\n\nWhether two picker rows collide cannot depend on a third row's width",
          "timestamp": "2026-08-24T13:45:38+01:00",
          "tree_id": "4bb95b10a05f0e26cb3e5eb02aec230e3aa4bd9a",
          "url": "https://github.com/blooop/devlaunch/commit/217f253b02ee16ef564662985f54268ef431f3fb"
        },
        "date": 1787575724098,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 0.890921,
            "range": "± 0.03846",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.256266,
            "range": "± 0.029044",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000029,
            "range": "± 0.000001",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.152189,
            "range": "± 0.066",
            "unit": "s",
            "extra": "runs=5/5 wall=1.154154s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 0.96419,
            "range": "± 0.046161",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.10476,
            "range": "± 0.114201",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.25444,
            "range": "± 0.017758",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.409074,
            "range": "± 0.068799",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.805602,
            "range": "± 0.154691",
            "unit": "s",
            "extra": "runs=5/5 wall=8.807958s v0.26.1, Linux-X64"
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
          "id": "943ec2d9fb6a8ac4a08e5886ea081c2d06d0cdd6",
          "message": "Merge pull request #382 from blooop/fix/verdict-cache-remembers-its-switches\n\nThe verdict cache remembers which switches the pass ran under",
          "timestamp": "2026-08-24T13:47:49+01:00",
          "tree_id": "a7063fa6f69961109eaa87c4fffb003d14ef1b1e",
          "url": "https://github.com/blooop/devlaunch/commit/943ec2d9fb6a8ac4a08e5886ea081c2d06d0cdd6"
        },
        "date": 1787575913380,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.358819,
            "range": "± 0.138535",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.326747,
            "range": "± 0.002438",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.00004,
            "range": "± 0.000017",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.687763,
            "range": "± 0.137797",
            "unit": "s",
            "extra": "runs=5/5 wall=1.689819s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.246835,
            "range": "± 0.086128",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 4.089709,
            "range": "± 0.579951",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.359993,
            "range": "± 0.015223",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.27946,
            "range": "± 0.113224",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.774055,
            "range": "± 0.47114",
            "unit": "s",
            "extra": "runs=5/5 wall=10.776282s v0.26.1, Linux-X64"
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
          "id": "d1d3f7d894ba6609634c6cc3c21dcd5b3a0b4daa",
          "message": "Merge pull request #383 from blooop/fix/chezmoi-recovery-cannot-destroy-recovery\n\nThe chezmoi recovery can no longer destroy what it recovers",
          "timestamp": "2026-08-24T13:49:00+01:00",
          "tree_id": "fa5a96c9f6108a554f2d84c12d497b9b67deacb3",
          "url": "https://github.com/blooop/devlaunch/commit/d1d3f7d894ba6609634c6cc3c21dcd5b3a0b4daa"
        },
        "date": 1787576104118,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.261645,
            "range": "± 0.109423",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.335855,
            "range": "± 0.010642",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000042,
            "range": "± 0.000003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.615717,
            "range": "± 0.10835",
            "unit": "s",
            "extra": "runs=5/5 wall=1.61775s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.352331,
            "range": "± 0.080063",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.238841,
            "range": "± 0.684005",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.334583,
            "range": "± 0.005231",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.290793,
            "range": "± 0.089921",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.081518,
            "range": "± 0.677624",
            "unit": "s",
            "extra": "runs=5/5 wall=10.083996s v0.26.1, Linux-X64"
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
          "id": "96e50507a1a252cb2ae1de4297eafebe2a53cea3",
          "message": "Merge pull request #384 from blooop/release/0.13.0\n\nRelease 0.13.0",
          "timestamp": "2026-08-24T13:57:08+01:00",
          "tree_id": "19c372b89c5302d095066f8a0bcb7d615965b218",
          "url": "https://github.com/blooop/devlaunch/commit/96e50507a1a252cb2ae1de4297eafebe2a53cea3"
        },
        "date": 1787576422651,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.269894,
            "range": "± 0.134167",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.329194,
            "range": "± 0.003217",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000044,
            "range": "± 0.000038",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.595964,
            "range": "± 0.135205",
            "unit": "s",
            "extra": "runs=5/5 wall=1.598132s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.277876,
            "range": "± 0.054354",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 4.177006,
            "range": "± 0.969114",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.329087,
            "range": "± 0.006762",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.255702,
            "range": "± 0.223597",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.963734,
            "range": "± 1.163843",
            "unit": "s",
            "extra": "runs=5/5 wall=10.966358s v0.26.1, Linux-X64"
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
          "id": "a8940e64f6cb1d7c5c003fff5cac7b79192e8d11",
          "message": "Merge pull request #378 from blooop/fix/external-review-quota-guard\n\nCI fails when the external reviewer did not actually review",
          "timestamp": "2026-08-24T14:04:16+01:00",
          "tree_id": "ecb704dd3a4178faf1e1cc4928f6f6b976d966ff",
          "url": "https://github.com/blooop/devlaunch/commit/a8940e64f6cb1d7c5c003fff5cac7b79192e8d11"
        },
        "date": 1787576827836,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.120416,
            "range": "± 0.032858",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.281514,
            "range": "± 0.011159",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.00004,
            "range": "± 0",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.416653,
            "range": "± 0.030257",
            "unit": "s",
            "extra": "runs=5/5 wall=1.418745s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.11661,
            "range": "± 0.068499",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.04089,
            "range": "± 0.57651",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.178071,
            "range": "± 0.005834",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.86037,
            "range": "± 0.176034",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.547118,
            "range": "± 0.488166",
            "unit": "s",
            "extra": "runs=5/5 wall=9.549271s v0.26.1, Linux-X64"
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
          "id": "9392779bab3f5ad86b3280842195e51a71caceb4",
          "message": "Merge pull request #385 from blooop/fix/self-review-satisfies-the-gate\n\nThe review gate asks whether the code was reviewed, not whether Sourcery answered",
          "timestamp": "2026-08-24T15:34:28+01:00",
          "tree_id": "07680921b7adc9afa3a0110e6387ab435a0bf994",
          "url": "https://github.com/blooop/devlaunch/commit/9392779bab3f5ad86b3280842195e51a71caceb4"
        },
        "date": 1787582240773,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.02854,
            "range": "± 0.111413",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.245777,
            "range": "± 0.051928",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000039,
            "range": "± 0.000003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.291882,
            "range": "± 0.10672",
            "unit": "s",
            "extra": "runs=5/5 wall=1.293681s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.041866,
            "range": "± 0.136569",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.082071,
            "range": "± 0.501487",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.232223,
            "range": "± 0.027319",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.528685,
            "range": "± 0.30802",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.397994,
            "range": "± 0.527692",
            "unit": "s",
            "extra": "runs=5/5 wall=9.401086s v0.26.1, Linux-X64"
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
          "id": "57955a3e61f56abd9c46394249092b949de717f1",
          "message": "Merge pull request #386 from blooop/docs/unslop-readme\n\ndocs: make the README readable — cut 24k words to 2.5k, move reference into docs/",
          "timestamp": "2026-08-24T18:25:29+01:00",
          "tree_id": "cda7c1019448485dae6cad6fb8f1886375a7ee3d",
          "url": "https://github.com/blooop/devlaunch/commit/57955a3e61f56abd9c46394249092b949de717f1"
        },
        "date": 1787592519987,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.299615,
            "range": "± 0.067332",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.33294,
            "range": "± 0.010115",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000043,
            "range": "± 0.000004",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.650934,
            "range": "± 0.068004",
            "unit": "s",
            "extra": "runs=5/5 wall=1.653221s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.271671,
            "range": "± 0.078385",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.281983,
            "range": "± 0.533595",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.371462,
            "range": "± 0.043761",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.239003,
            "range": "± 0.172209",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.634582,
            "range": "± 0.456844",
            "unit": "s",
            "extra": "runs=5/5 wall=10.636864s v0.26.1, Linux-X64"
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
          "id": "738918b2d4fa6c22fd4ec60317d2515fdabf9524",
          "message": "Merge pull request #414 from blooop/fix/aid-pty-flake\n\nA paste the test delivers in two writes is not a paste",
          "timestamp": "2026-08-25T12:07:57+01:00",
          "tree_id": "c276654ed05157010bf1d6fa5fb1eeebf6aee878",
          "url": "https://github.com/blooop/devlaunch/commit/738918b2d4fa6c22fd4ec60317d2515fdabf9524"
        },
        "date": 1787656271108,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.339195,
            "range": "± 0.079497",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.347564,
            "range": "± 0.005564",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000041,
            "range": "± 0.000003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.682475,
            "range": "± 0.082554",
            "unit": "s",
            "extra": "runs=5/5 wall=1.68483s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.409129,
            "range": "± 0.10188",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.370125,
            "range": "± 0.567316",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.250727,
            "range": "± 0.047278",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.393702,
            "range": "± 0.168251",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.659688,
            "range": "± 0.597768",
            "unit": "s",
            "extra": "runs=5/5 wall=10.661998s v0.26.1, Linux-X64"
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
          "id": "449cae2f71b67c4c28cf3ef17523fddb4013cc9b",
          "message": "Merge pull request #430 from blooop/test/liveness-pins\n\nPin the launch shapes that had no liveness pin",
          "timestamp": "2026-08-25T12:17:16+01:00",
          "tree_id": "f2f42c8dead394bf308d1bec8d5bf254d9d05885",
          "url": "https://github.com/blooop/devlaunch/commit/449cae2f71b67c4c28cf3ef17523fddb4013cc9b"
        },
        "date": 1787656808864,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.14132,
            "range": "± 0.021409",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.270633,
            "range": "± 0.00954",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000041,
            "range": "± 0.000004",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.407986,
            "range": "± 0.027507",
            "unit": "s",
            "extra": "runs=5/5 wall=1.410185s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.17526,
            "range": "± 0.164747",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.948292,
            "range": "± 0.901935",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.168549,
            "range": "± 0.010699",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.769542,
            "range": "± 0.092341",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.875438,
            "range": "± 0.960211",
            "unit": "s",
            "extra": "runs=5/5 wall=9.877345s v0.26.1, Linux-X64"
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
          "id": "c978b937a62a44e70e5a12b716589a4df0d7b3e1",
          "message": "Merge pull request #415 from blooop/docs/runner-seam-and-trips\n\nSay what Trips is, replace the Runner impl count with the shape, and guard the count",
          "timestamp": "2026-08-25T12:25:55+01:00",
          "tree_id": "fbfee37a8f5012588319f2b6ad2a229a8e6db88c",
          "url": "https://github.com/blooop/devlaunch/commit/c978b937a62a44e70e5a12b716589a4df0d7b3e1"
        },
        "date": 1787657333609,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.235621,
            "range": "± 0.074737",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.274457,
            "range": "± 0.098336",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000042,
            "range": "± 0.000006",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.547207,
            "range": "± 0.151296",
            "unit": "s",
            "extra": "runs=5/5 wall=1.549469s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.116446,
            "range": "± 0.202843",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.145231,
            "range": "± 0.837693",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.175381,
            "range": "± 0.007679",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.995922,
            "range": "± 0.048867",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.631189,
            "range": "± 0.741985",
            "unit": "s",
            "extra": "runs=5/5 wall=9.633474s v0.26.1, Linux-X64"
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
          "id": "c437fc7aace905a9399bb1252a40da749d327c42",
          "message": "Merge pull request #407 from blooop/fix_arch\n\nDEVLAUNCH_NO_TTY has one reading again, not two",
          "timestamp": "2026-08-25T12:44:35+01:00",
          "tree_id": "c909cc883ffe15a5f7563bced83c600f9b3e37ab",
          "url": "https://github.com/blooop/devlaunch/commit/c437fc7aace905a9399bb1252a40da749d327c42"
        },
        "date": 1787658459303,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.025304,
            "range": "± 0.126819",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.249128,
            "range": "± 0.065693",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000024,
            "range": "± 0.000001",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.241931,
            "range": "± 0.187697",
            "unit": "s",
            "extra": "runs=5/5 wall=1.243597s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 0.99249,
            "range": "± 0.095734",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.024598,
            "range": "± 0.418456",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.298827,
            "range": "± 0.02904",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.965253,
            "range": "± 0.153088",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.340717,
            "range": "± 0.356253",
            "unit": "s",
            "extra": "runs=5/5 wall=8.34285s v0.26.1, Linux-X64"
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
          "id": "6ce6bdb196e5451db3392c1052b8f7ab4fb678fb",
          "message": "Merge pull request #413 from blooop/fix/aid-agent-env\n\nAn undecodable DEVLAUNCH_AID_AGENT is a name to refuse, not an unset variable",
          "timestamp": "2026-08-25T13:01:07+01:00",
          "tree_id": "aeb69bcfdabe378448fe8eea2c1f28dbe86ae299",
          "url": "https://github.com/blooop/devlaunch/commit/6ce6bdb196e5451db3392c1052b8f7ab4fb678fb"
        },
        "date": 1787659449517,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.224163,
            "range": "± 0.045526",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.27277,
            "range": "± 0.022736",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000059,
            "range": "± 0.000023",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.499182,
            "range": "± 0.063305",
            "unit": "s",
            "extra": "runs=5/5 wall=1.501516s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.155957,
            "range": "± 1.01111",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.074071,
            "range": "± 0.436484",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.180347,
            "range": "± 0.008686",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.012403,
            "range": "± 0.128034",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.412117,
            "range": "± 1.55301",
            "unit": "s",
            "extra": "runs=5/5 wall=9.41437s v0.26.1, Linux-X64"
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
          "id": "a17e03064d224bda744c8ddd79cbef1727ebe982",
          "message": "Merge pull request #429 from blooop/fix/git-refusal-classification\n\nA git refusal is read once, where git's words already are",
          "timestamp": "2026-08-25T13:12:51+01:00",
          "tree_id": "92db474a886e430eac14fb1cd5a013200d77197d",
          "url": "https://github.com/blooop/devlaunch/commit/a17e03064d224bda744c8ddd79cbef1727ebe982"
        },
        "date": 1787660147625,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.094243,
            "range": "± 0.129203",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.26399,
            "range": "± 0.008964",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000041,
            "range": "± 0.000035",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.35855,
            "range": "± 0.129217",
            "unit": "s",
            "extra": "runs=5/5 wall=1.360732s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.127623,
            "range": "± 0.029076",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.952419,
            "range": "± 0.715434",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.185441,
            "range": "± 0.010215",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.827985,
            "range": "± 0.109937",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.068231,
            "range": "± 0.644297",
            "unit": "s",
            "extra": "runs=5/5 wall=10.073026s v0.26.1, Linux-X64"
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
          "id": "8fc5abcba4e528a37c1b7777b9968ffaeb6b36c9",
          "message": "Merge pull request #440 from blooop/wayfinder/devlaunch-436\n\nfix: put claude's title suppression where every session reads it",
          "timestamp": "2026-08-25T13:20:24+01:00",
          "tree_id": "28c862e22258a036a36a2f39788d51af9bd0eaa0",
          "url": "https://github.com/blooop/devlaunch/commit/8fc5abcba4e528a37c1b7777b9968ffaeb6b36c9"
        },
        "date": 1787660616575,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.371408,
            "range": "± 0.024368",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.350152,
            "range": "± 0.081494",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000041,
            "range": "± 0.000003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.721009,
            "range": "± 0.099336",
            "unit": "s",
            "extra": "runs=5/5 wall=1.723144s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.36459,
            "range": "± 0.075111",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 4.134434,
            "range": "± 0.57335",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.273504,
            "range": "± 0.014713",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.28941,
            "range": "± 0.028763",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.923385,
            "range": "± 0.54202",
            "unit": "s",
            "extra": "runs=5/5 wall=10.925679s v0.26.1, Linux-X64"
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
          "id": "60983562033740ad8c26193e1e8c7e629b5dfc29",
          "message": "Merge pull request #443 from blooop/release/0.14.0\n\nRelease 0.14.0",
          "timestamp": "2026-08-25T13:25:52+01:00",
          "tree_id": "9091a6697a9ef816fa89836ac14996660f2fe79a",
          "url": "https://github.com/blooop/devlaunch/commit/60983562033740ad8c26193e1e8c7e629b5dfc29"
        },
        "date": 1787660959222,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.099855,
            "range": "± 0.159664",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.233384,
            "range": "± 0.045677",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000025,
            "range": "± 0.000017",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.351647,
            "range": "± 0.15258",
            "unit": "s",
            "extra": "runs=5/5 wall=1.353197s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 0.975662,
            "range": "± 0.413189",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.973697,
            "range": "± 0.621676",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.326913,
            "range": "± 0.123037",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.257512,
            "range": "± 0.392677",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.725918,
            "range": "± 0.753123",
            "unit": "s",
            "extra": "runs=5/5 wall=9.727932s v0.26.1, Linux-X64"
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
          "id": "095429de8557ff5cc53699b53992d43a03af22f1",
          "message": "Merge pull request #428 from blooop/refactor/metadata-update-closures\n\nA field change no longer crosses the metadata seam",
          "timestamp": "2026-08-25T13:30:51+01:00",
          "tree_id": "8a97d89e9e8e60eea2efdf74adb4af24307c62f2",
          "url": "https://github.com/blooop/devlaunch/commit/095429de8557ff5cc53699b53992d43a03af22f1"
        },
        "date": 1787661237979,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.40346,
            "range": "± 0.138364",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.340641,
            "range": "± 0.150882",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000041,
            "range": "± 0.000004",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.86888,
            "range": "± 0.168099",
            "unit": "s",
            "extra": "runs=5/5 wall=1.871149s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.305009,
            "range": "± 0.080994",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.906495,
            "range": "± 0.430566",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.356843,
            "range": "± 0.010256",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.32465,
            "range": "± 0.131074",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.862738,
            "range": "± 0.466087",
            "unit": "s",
            "extra": "runs=5/5 wall=10.865098s v0.26.1, Linux-X64"
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
          "id": "2aa9602b37826e71bf83b349041aa6ddab040f0c",
          "message": "Merge pull request #427 from blooop/refactor/devpod-home-adapter\n\ndevpod's on-disk layout gets the module it was missing",
          "timestamp": "2026-08-25T13:52:03+01:00",
          "tree_id": "5d2de5d61e749caadd68085c5d9813b0160255e8",
          "url": "https://github.com/blooop/devlaunch/commit/2aa9602b37826e71bf83b349041aa6ddab040f0c"
        },
        "date": 1787662549290,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.310614,
            "range": "± 0.148566",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.315169,
            "range": "± 0.032456",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.00004,
            "range": "± 0.000005",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.695075,
            "range": "± 0.154516",
            "unit": "s",
            "extra": "runs=5/5 wall=1.697237s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.199035,
            "range": "± 1.272621",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.325112,
            "range": "± 0.833431",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.350114,
            "range": "± 0.021396",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.122083,
            "range": "± 0.183459",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.662446,
            "range": "± 1.287861",
            "unit": "s",
            "extra": "runs=5/5 wall=10.664889s v0.26.1, Linux-X64"
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
          "id": "1ab64d8c39b7ec1d31f963288291ab0ea419b3af",
          "message": "Merge pull request #433 from blooop/fix/ssh-config-path\n\ndl looks for devpod's ssh config where devpod writes it",
          "timestamp": "2026-08-25T14:08:59+01:00",
          "tree_id": "a8498fc967a318f8a6a29ba5478337a88de696ea",
          "url": "https://github.com/blooop/devlaunch/commit/1ab64d8c39b7ec1d31f963288291ab0ea419b3af"
        },
        "date": 1787663526531,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.326409,
            "range": "± 0.060738",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.333331,
            "range": "± 0.005013",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000027,
            "range": "± 0.000006",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.66063,
            "range": "± 0.059775",
            "unit": "s",
            "extra": "runs=5/5 wall=1.662748s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.326974,
            "range": "± 0.067749",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.245137,
            "range": "± 0.196003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.368283,
            "range": "± 0.014671",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 5.333234,
            "range": "± 0.083501",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.32461,
            "range": "± 0.150084",
            "unit": "s",
            "extra": "runs=5/5 wall=10.3265s v0.26.1, Linux-X64"
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
          "id": "e96631db4a5b40d9541d3b7dbe6ff4e5c393eb0e",
          "message": "Merge pull request #425 from blooop/feat/zellij-opt-in\n\nGate the zellij stage on DEVLAUNCH_ZELLIJ; retire DEVLAUNCH_NO_ZELLIJ",
          "timestamp": "2026-08-25T14:30:49+01:00",
          "tree_id": "a68170b233250cdad31daba43be338c55df1206c",
          "url": "https://github.com/blooop/devlaunch/commit/e96631db4a5b40d9541d3b7dbe6ff4e5c393eb0e"
        },
        "date": 1787664808710,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.332854,
            "range": "± 0.184719",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.302908,
            "range": "± 0.033659",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000048,
            "range": "± 0.000005",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.701384,
            "range": "± 0.190458",
            "unit": "s",
            "extra": "runs=5/5 wall=1.703527s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.174477,
            "range": "± 0.06008",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.967279,
            "range": "± 0.090221",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.207786,
            "range": "± 0.45827",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.626925,
            "range": "± 0.190619",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.103508,
            "range": "± 0.546815",
            "unit": "s",
            "extra": "runs=5/5 wall=8.105695s v0.26.1, Linux-X64"
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
          "id": "ce37869129b90013df816c36cf16e62add3492ba",
          "message": "Merge pull request #449 from blooop/branchlength\n\nOne name for a workspace, 47 chars, everywhere (0.15.0)",
          "timestamp": "2026-08-25T16:29:29+01:00",
          "tree_id": "5d7448416a1a5a77960b11baa0e1ee78c66870ac",
          "url": "https://github.com/blooop/devlaunch/commit/ce37869129b90013df816c36cf16e62add3492ba"
        },
        "date": 1787671938989,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.223335,
            "range": "± 0.089733",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.260446,
            "range": "± 0.026954",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000051,
            "range": "± 0.000005",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.493167,
            "range": "± 0.100357",
            "unit": "s",
            "extra": "runs=5/5 wall=1.495721s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.163567,
            "range": "± 0.08679",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.008595,
            "range": "± 0.449121",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.266899,
            "range": "± 0.06416",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.758891,
            "range": "± 0.305293",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.249595,
            "range": "± 0.415049",
            "unit": "s",
            "extra": "runs=5/5 wall=8.251789s v0.26.1, Linux-X64"
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
          "id": "1f5c07514d5f568af806e8647606fc21724c8b31",
          "message": "Merge pull request #466 from blooop/docs/compose-network-address-pools\n\nDocument the address-pool ceiling a compose workspace launch hits",
          "timestamp": "2026-08-25T17:41:09+01:00",
          "tree_id": "5a426efc1cb067bf2e0dc3b99d6c7bf7fcbf489f",
          "url": "https://github.com/blooop/devlaunch/commit/1f5c07514d5f568af806e8647606fc21724c8b31"
        },
        "date": 1787676238921,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.189276,
            "range": "± 0.057443",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.275533,
            "range": "± 0.020157",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000042,
            "range": "± 0.000001",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.449192,
            "range": "± 0.057829",
            "unit": "s",
            "extra": "runs=5/5 wall=1.451398s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.211765,
            "range": "± 0.085977",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.280544,
            "range": "± 0.486762",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.834531,
            "range": "± 0.431907",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.333519,
            "range": "± 0.171068",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.422443,
            "range": "± 0.609704",
            "unit": "s",
            "extra": "runs=5/5 wall=9.424536s v0.26.1, Linux-X64"
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
          "id": "90702b570cdf84c6f2417acc6124e9428c95a035",
          "message": "Merge pull request #457 from blooop/rm_cmd\n\nName the workspace a delete is removing, and release 0.16.0",
          "timestamp": "2026-08-25T17:55:52+01:00",
          "tree_id": "fc2c577ba36b5bee65fd73fea05d7cedbb5cb10b",
          "url": "https://github.com/blooop/devlaunch/commit/90702b570cdf84c6f2417acc6124e9428c95a035"
        },
        "date": 1787677131679,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.349641,
            "range": "± 0.09187",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.322571,
            "range": "± 0.005883",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000044,
            "range": "± 0.000006",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.668256,
            "range": "± 0.091343",
            "unit": "s",
            "extra": "runs=5/5 wall=1.670371s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.271087,
            "range": "± 0.117873",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.427315,
            "range": "± 0.85959",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.372466,
            "range": "± 0.165105",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.866321,
            "range": "± 0.079507",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.220733,
            "range": "± 0.982153",
            "unit": "s",
            "extra": "runs=5/5 wall=9.222991s v0.26.1, Linux-X64"
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
          "id": "e55239a4c913cc988c5b4b5a3e595d0daa9a58dc",
          "message": "Merge pull request #474 from blooop/wayfinder/devlaunch-471\n\nAsk the clone what it holds, not the branch it is on",
          "timestamp": "2026-08-25T18:16:05+01:00",
          "tree_id": "6514e9c38a6c8f6cdd9420105980784d98ae4ff5",
          "url": "https://github.com/blooop/devlaunch/commit/e55239a4c913cc988c5b4b5a3e595d0daa9a58dc"
        },
        "date": 1787678341196,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 0.993896,
            "range": "± 0.071263",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.254097,
            "range": "± 0.027421",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000028,
            "range": "± 0.000003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.240127,
            "range": "± 0.064659",
            "unit": "s",
            "extra": "runs=5/5 wall=1.241963s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 0.972825,
            "range": "± 0.055085",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.067962,
            "range": "± 0.242116",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.654108,
            "range": "± 0.325682",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.094927,
            "range": "± 0.070565",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 7.589473,
            "range": "± 0.445963",
            "unit": "s",
            "extra": "runs=5/5 wall=7.591413s v0.26.1, Linux-X64"
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
          "id": "35a9fe69deb7a34edbe71d2ad92016ddd6c2448b",
          "message": "Merge pull request #473 from blooop/docs/additive-address-pools\n\nKeep the stock 172.17 pool in front of the widened one",
          "timestamp": "2026-08-25T18:21:33+01:00",
          "tree_id": "043052899ca158bc0cad05dca1d46d8d21381d72",
          "url": "https://github.com/blooop/devlaunch/commit/35a9fe69deb7a34edbe71d2ad92016ddd6c2448b"
        },
        "date": 1787678658119,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.147491,
            "range": "± 0.036478",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.30105,
            "range": "± 0.019086",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000049,
            "range": "± 0.00001",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.458667,
            "range": "± 0.044533",
            "unit": "s",
            "extra": "runs=5/5 wall=1.461411s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.164947,
            "range": "± 0.052011",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.114945,
            "range": "± 0.490141",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.585256,
            "range": "± 0.057516",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.572555,
            "range": "± 0.163338",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.552238,
            "range": "± 0.460094",
            "unit": "s",
            "extra": "runs=5/5 wall=8.554359s v0.26.1, Linux-X64"
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
          "id": "a5c00491d2528599fabe5afbc9431493e0d53b8f",
          "message": "Merge pull request #476 from blooop/wayfinder/devlaunch-464\n\nFetch tags with a forced refspec",
          "timestamp": "2026-08-25T18:45:16+01:00",
          "tree_id": "8fa04245199bd9098260c00218ecf9105e8928be",
          "url": "https://github.com/blooop/devlaunch/commit/a5c00491d2528599fabe5afbc9431493e0d53b8f"
        },
        "date": 1787680081399,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.10994,
            "range": "± 0.101583",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.265844,
            "range": "± 0.0161",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000047,
            "range": "± 0.000005",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.372272,
            "range": "± 0.112439",
            "unit": "s",
            "extra": "runs=5/5 wall=1.374368s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.128508,
            "range": "± 0.127644",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.105305,
            "range": "± 0.574104",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.177374,
            "range": "± 0.003832",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.564338,
            "range": "± 1.102939",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.098474,
            "range": "± 1.26124",
            "unit": "s",
            "extra": "runs=5/5 wall=8.100923s v0.26.1, Linux-X64"
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
          "id": "bce26d1a8f159e2ebed57b018807aafa2def660e",
          "message": "Merge pull request #479 from blooop/tab_titel\n\nTab reads devlaunch@main; the picker never gives up a branch",
          "timestamp": "2026-08-25T19:16:49+01:00",
          "tree_id": "ffa6da57e5695031fa8359948f7394a7a897f886",
          "url": "https://github.com/blooop/devlaunch/commit/bce26d1a8f159e2ebed57b018807aafa2def660e"
        },
        "date": 1787681985301,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.165409,
            "range": "± 0.115481",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.257341,
            "range": "± 0.112974",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000042,
            "range": "± 0.000004",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.53136,
            "range": "± 0.136231",
            "unit": "s",
            "extra": "runs=5/5 wall=1.533123s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.266569,
            "range": "± 0.188246",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.873535,
            "range": "± 0.186763",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.912216,
            "range": "± 0.405782",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.210945,
            "range": "± 0.170694",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.444525,
            "range": "± 0.396453",
            "unit": "s",
            "extra": "runs=5/5 wall=8.446385s v0.26.1, Linux-X64"
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
          "id": "5cb909acb2b6fceafc0d3f4507a858270147430d",
          "message": "Merge pull request #478 from blooop/wayfinder/devlaunch-470\n\nPack the bare caches' refs on the sweep",
          "timestamp": "2026-08-25T19:25:57+01:00",
          "tree_id": "1904bc6f5614e7764c261e790fb79497a77431e9",
          "url": "https://github.com/blooop/devlaunch/commit/5cb909acb2b6fceafc0d3f4507a858270147430d"
        },
        "date": 1787682542905,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.376066,
            "range": "± 0.05707",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.348757,
            "range": "± 0.006197",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000041,
            "range": "± 0.000001",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.722057,
            "range": "± 0.061629",
            "unit": "s",
            "extra": "runs=5/5 wall=1.724286s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.311694,
            "range": "± 0.066947",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 4.249094,
            "range": "± 0.514804",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.363999,
            "range": "± 0.363589",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.810517,
            "range": "± 0.097833",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.946201,
            "range": "± 0.745663",
            "unit": "s",
            "extra": "runs=5/5 wall=9.948418s v0.26.1, Linux-X64"
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
          "id": "16605f8b1a1de945e440d6b85d26176d5785c482",
          "message": "Merge pull request #481 from blooop/wayfinder/devlaunch-467\n\nRetire repos_dir",
          "timestamp": "2026-08-25T20:09:40+01:00",
          "tree_id": "9cb9acf413ac2b48d3547e26c071b37b62be8935",
          "url": "https://github.com/blooop/devlaunch/commit/16605f8b1a1de945e440d6b85d26176d5785c482"
        },
        "date": 1787685155028,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.423195,
            "range": "± 0.099911",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.347152,
            "range": "± 0.016493",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000047,
            "range": "± 0.000004",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.755114,
            "range": "± 0.094219",
            "unit": "s",
            "extra": "runs=5/5 wall=1.757179s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.279961,
            "range": "± 0.046974",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.129529,
            "range": "± 0.556004",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.252914,
            "range": "± 0.043072",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.984664,
            "range": "± 0.139287",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.685203,
            "range": "± 0.657373",
            "unit": "s",
            "extra": "runs=5/5 wall=8.687767s v0.26.1, Linux-X64"
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
          "id": "d180c3f3f63b7de9649792bddfd9f901a18b0c4a",
          "message": "Merge pull request #482 from blooop/ags/completion-specs-first\n\nComplete owners before workspace ids",
          "timestamp": "2026-08-25T20:55:56+01:00",
          "tree_id": "16cb1afb01b43362bac7f627c641561d1e4b36b3",
          "url": "https://github.com/blooop/devlaunch/commit/d180c3f3f63b7de9649792bddfd9f901a18b0c4a"
        },
        "date": 1787687949149,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.311039,
            "range": "± 0.136025",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.338803,
            "range": "± 0.010849",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000044,
            "range": "± 0.000013",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.66647,
            "range": "± 0.144113",
            "unit": "s",
            "extra": "runs=5/5 wall=1.668826s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.301082,
            "range": "± 0.096131",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 4.19199,
            "range": "± 0.848039",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.346707,
            "range": "± 0.029677",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.887648,
            "range": "± 0.084959",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.818659,
            "range": "± 0.889857",
            "unit": "s",
            "extra": "runs=5/5 wall=9.820831s v0.26.1, Linux-X64"
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
          "id": "d44971058b951b35bd7ecf27f5fa914ec2a28dc9",
          "message": "Merge pull request #483 from blooop/fixed_width\n\nThe selector is a table, headings and all",
          "timestamp": "2026-08-25T22:41:47+01:00",
          "tree_id": "cabbafa8d5caf14fbb55e3606a8f9447762b2afa",
          "url": "https://github.com/blooop/devlaunch/commit/d44971058b951b35bd7ecf27f5fa914ec2a28dc9"
        },
        "date": 1787694305270,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.659564,
            "range": "± 0.138666",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.486616,
            "range": "± 0.108839",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000043,
            "range": "± 0.000006",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 2.04941,
            "range": "± 0.21293",
            "unit": "s",
            "extra": "runs=5/5 wall=2.051709s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.278199,
            "range": "± 0.406754",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.455773,
            "range": "± 0.487484",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.347209,
            "range": "± 0.008771",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.822916,
            "range": "± 0.216092",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.427762,
            "range": "± 0.389133",
            "unit": "s",
            "extra": "runs=5/5 wall=9.430148s v0.26.1, Linux-X64"
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
          "id": "cdd31a2620e52ce3696a1d8787d9b2d4495f8677",
          "message": "Merge pull request #486 from blooop/fix/unpushed-guard-counts-tags\n\nThe delete guard stops counting tags the remote carries",
          "timestamp": "2026-08-26T12:13:34+01:00",
          "tree_id": "cc0087dfb92c1a2290e156d491755d9977d4c144",
          "url": "https://github.com/blooop/devlaunch/commit/cdd31a2620e52ce3696a1d8787d9b2d4495f8677"
        },
        "date": 1787742991361,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.446336,
            "range": "± 0.126049",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.424953,
            "range": "± 0.112761",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000046,
            "range": "± 0.000004",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.896379,
            "range": "± 0.15391",
            "unit": "s",
            "extra": "runs=5/5 wall=1.898526s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.326906,
            "range": "± 0.158503",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.318937,
            "range": "± 0.377922",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.385518,
            "range": "± 0.016525",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.961596,
            "range": "± 0.095657",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.090671,
            "range": "± 0.42574",
            "unit": "s",
            "extra": "runs=5/5 wall=9.092896s v0.26.1, Linux-X64"
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
          "id": "018ab950bdafdfa5b405a6e5b0c8ad9c4c3caa9d",
          "message": "Merge pull request #489 from blooop/wayfinder/devlaunch-484\n\ndl <ws> kill: the verb for a workspace that will not answer",
          "timestamp": "2026-08-26T12:59:04+01:00",
          "tree_id": "1dcd5eaa5c6e70aa2eebdf93d15578e81f12b48a",
          "url": "https://github.com/blooop/devlaunch/commit/018ab950bdafdfa5b405a6e5b0c8ad9c4c3caa9d"
        },
        "date": 1787745731391,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.25515,
            "range": "± 0.173389",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.310034,
            "range": "± 0.03406",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000039,
            "range": "± 0.000005",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.60838,
            "range": "± 0.172023",
            "unit": "s",
            "extra": "runs=5/5 wall=1.61015s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.14735,
            "range": "± 0.117139",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.51143,
            "range": "± 0.405045",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.360375,
            "range": "± 0.024173",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.599767,
            "range": "± 0.056267",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.768861,
            "range": "± 0.434574",
            "unit": "s",
            "extra": "runs=5/5 wall=8.770856s v0.26.1, Linux-X64"
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
          "id": "2f98b3f8df510197ade3b71f64f153789b3db9c7",
          "message": "Merge pull request #490 from blooop/release/0.20.0\n\nCut 0.20.0",
          "timestamp": "2026-08-26T13:05:20+01:00",
          "tree_id": "6065793bc0239dce0ece13c375e97c31af5add0a",
          "url": "https://github.com/blooop/devlaunch/commit/2f98b3f8df510197ade3b71f64f153789b3db9c7"
        },
        "date": 1787746102667,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.332097,
            "range": "± 0.05663",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.342818,
            "range": "± 0.007381",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000043,
            "range": "± 0.000003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.687977,
            "range": "± 0.058159",
            "unit": "s",
            "extra": "runs=5/5 wall=1.690254s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.286722,
            "range": "± 0.091043",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.212821,
            "range": "± 0.4165",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.402036,
            "range": "± 0.442793",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.816811,
            "range": "± 0.10489",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.938495,
            "range": "± 0.580016",
            "unit": "s",
            "extra": "runs=5/5 wall=8.940714s v0.26.1, Linux-X64"
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
          "id": "34c3f42a8180c8d03f6c6a3120efd099195301e7",
          "message": "Merge pull request #493 from blooop/remote_control\n\naid --remote-control: the session you can pick up on a phone",
          "timestamp": "2026-08-28T19:47:06+01:00",
          "tree_id": "e2218afc4116e44581465f4765ff391812ef3994",
          "url": "https://github.com/blooop/devlaunch/commit/34c3f42a8180c8d03f6c6a3120efd099195301e7"
        },
        "date": 1787943003516,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.218062,
            "range": "± 0.14285",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.325081,
            "range": "± 0.026504",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000047,
            "range": "± 0.000021",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.569553,
            "range": "± 0.157276",
            "unit": "s",
            "extra": "runs=5/5 wall=1.571716s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.235262,
            "range": "± 0.201829",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.289002,
            "range": "± 0.404051",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.347003,
            "range": "± 0.004423",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.934858,
            "range": "± 0.287386",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.158829,
            "range": "± 0.580197",
            "unit": "s",
            "extra": "runs=5/5 wall=9.160926s v0.26.1, Linux-X64"
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
          "id": "bbbadbdae6b6ce517070dc008dc5dd326e0a5b0e",
          "message": "Merge pull request #494 from blooop/release/0.21.0\n\nCut 0.21.0",
          "timestamp": "2026-08-28T19:51:23+01:00",
          "tree_id": "da4ef726fa6f9b5d1acd9e19e5adb7b5bddada1b",
          "url": "https://github.com/blooop/devlaunch/commit/bbbadbdae6b6ce517070dc008dc5dd326e0a5b0e"
        },
        "date": 1787943253410,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.157915,
            "range": "± 0.175233",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.269965,
            "range": "± 0.09324",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000056,
            "range": "± 0.000011",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.429035,
            "range": "± 0.195352",
            "unit": "s",
            "extra": "runs=5/5 wall=1.431165s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.113343,
            "range": "± 0.199397",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.971324,
            "range": "± 0.082502",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.273359,
            "range": "± 0.007496",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.625979,
            "range": "± 0.14671",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.074684,
            "range": "± 0.312766",
            "unit": "s",
            "extra": "runs=5/5 wall=8.076903s v0.26.1, Linux-X64"
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
          "id": "7a8d00f976d59fa6ec057982fdfe0ce2a7b277f0",
          "message": "Merge pull request #403 from blooop/docs/launch-timing-numbers-provenance\n\nThese launch seconds measured a build that no longer ships",
          "timestamp": "2026-08-28T19:52:00+01:00",
          "tree_id": "868cdf146e30f4a8e6fc03f13887dfc15d7bc31e",
          "url": "https://github.com/blooop/devlaunch/commit/7a8d00f976d59fa6ec057982fdfe0ce2a7b277f0"
        },
        "date": 1787943464485,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.281103,
            "range": "± 0.077629",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.324519,
            "range": "± 0.023443",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000026,
            "range": "± 0.000009",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.58065,
            "range": "± 0.084544",
            "unit": "s",
            "extra": "runs=5/5 wall=1.582201s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.27549,
            "range": "± 0.061961",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 4.615511,
            "range": "± 0.739802",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.404919,
            "range": "± 0.491468",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.461006,
            "range": "± 0.256003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 10.371839,
            "range": "± 0.760057",
            "unit": "s",
            "extra": "runs=5/5 wall=10.373808s v0.26.1, Linux-X64"
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
          "id": "3dc322d3ed8daa259df5b95dcb7b603d8237f910",
          "message": "Merge pull request #492 from blooop/rme\n\nrme: the delete, and then the shell",
          "timestamp": "2026-08-28T19:59:33+01:00",
          "tree_id": "f042346f3c2d0bca7571afbe98ccc9060b5d17bd",
          "url": "https://github.com/blooop/devlaunch/commit/3dc322d3ed8daa259df5b95dcb7b603d8237f910"
        },
        "date": 1787943728529,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 0.961284,
            "range": "± 0.062101",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.25634,
            "range": "± 0.023684",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000038,
            "range": "± 0.000003",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.232598,
            "range": "± 0.048141",
            "unit": "s",
            "extra": "runs=5/5 wall=1.234417s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.017649,
            "range": "± 0.069361",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 2.99925,
            "range": "± 0.091795",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.175231,
            "range": "± 0.044418",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 2.985857,
            "range": "± 0.076497",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 7.254155,
            "range": "± 0.166414",
            "unit": "s",
            "extra": "runs=5/5 wall=7.256098s v0.26.1, Linux-X64"
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
          "id": "4e43791a8607552582c2e569a69b5f74d931a33b",
          "message": "Merge pull request #491 from blooop/claude-identity-in-workspaces\n\nForward the host's Claude login into workspaces that have none",
          "timestamp": "2026-08-28T20:14:51+01:00",
          "tree_id": "d736930e5ad993735d0d1af59099cfdb6e6de3a1",
          "url": "https://github.com/blooop/devlaunch/commit/4e43791a8607552582c2e569a69b5f74d931a33b"
        },
        "date": 1787944659128,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.035463,
            "range": "± 0.078371",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.236236,
            "range": "± 0.033386",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000025,
            "range": "± 0.000002",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.282355,
            "range": "± 0.08559",
            "unit": "s",
            "extra": "runs=5/5 wall=1.284078s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 0.94575,
            "range": "± 0.061537",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.642778,
            "range": "± 0.553186",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.267458,
            "range": "± 0.007868",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 2.871986,
            "range": "± 0.140004",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 7.545909,
            "range": "± 0.528933",
            "unit": "s",
            "extra": "runs=5/5 wall=7.547747s v0.26.1, Linux-X64"
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
          "id": "21b5f35ca05bf444602c47744a97618d09cb86e7",
          "message": "Merge pull request #496 from blooop/remote-control-default\n\nRemote Control on by default, and the two switches that turn it off",
          "timestamp": "2026-08-28T21:06:14+01:00",
          "tree_id": "df3112c5249ed123dc40b7ea3a85b1e2ccf57514",
          "url": "https://github.com/blooop/devlaunch/commit/21b5f35ca05bf444602c47744a97618d09cb86e7"
        },
        "date": 1787947735786,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.176864,
            "range": "± 0.117442",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.251524,
            "range": "± 0.013673",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000048,
            "range": "± 0.000002",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.406024,
            "range": "± 0.114376",
            "unit": "s",
            "extra": "runs=5/5 wall=1.408118s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.053339,
            "range": "± 0.082088",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.064624,
            "range": "± 0.520489",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.249943,
            "range": "± 0.013511",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.367293,
            "range": "± 0.110031",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 7.693497,
            "range": "± 0.628813",
            "unit": "s",
            "extra": "runs=5/5 wall=7.695663s v0.26.1, Linux-X64"
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
          "id": "91eb6d17c20f3f405a26456ded49ba5aa853fb3c",
          "message": "Merge pull request #497 from blooop/release/0.23.0\n\nCut 0.23.0",
          "timestamp": "2026-08-28T21:10:24+01:00",
          "tree_id": "a85bcae8d624a33f4c8359d4914de6921f04ed2a",
          "url": "https://github.com/blooop/devlaunch/commit/91eb6d17c20f3f405a26456ded49ba5aa853fb3c"
        },
        "date": 1787948006117,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.418682,
            "range": "± 0.113148",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.365435,
            "range": "± 0.024418",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000057,
            "range": "± 0.000007",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.818853,
            "range": "± 0.114181",
            "unit": "s",
            "extra": "runs=5/5 wall=1.821026s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.422286,
            "range": "± 0.032685",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.559949,
            "range": "± 0.455004",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.250648,
            "range": "± 0.047643",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.137318,
            "range": "± 0.147073",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.611428,
            "range": "± 0.4326",
            "unit": "s",
            "extra": "runs=5/5 wall=9.613825s v0.26.1, Linux-X64"
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
          "id": "b2fad91cd51cc5208a222aff33d414d480c84ac6",
          "message": "Merge pull request #498 from blooop/kill_4real\n\nkill deletes the workspace it just unwedged",
          "timestamp": "2026-08-28T21:35:54+01:00",
          "tree_id": "9f019df695d55e944e8d6f77597094433906bb16",
          "url": "https://github.com/blooop/devlaunch/commit/b2fad91cd51cc5208a222aff33d414d480c84ac6"
        },
        "date": 1787949515773,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.323957,
            "range": "± 0.144093",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.280471,
            "range": "± 0.008057",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000048,
            "range": "± 0.000004",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.602972,
            "range": "± 0.146274",
            "unit": "s",
            "extra": "runs=5/5 wall=1.605159s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.178372,
            "range": "± 0.126222",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.09242,
            "range": "± 0.14565",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.172984,
            "range": "± 0.017751",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.739119,
            "range": "± 0.154273",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.188076,
            "range": "± 0.38984",
            "unit": "s",
            "extra": "runs=5/5 wall=8.190647s v0.26.1, Linux-X64"
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
          "id": "9da6255fb0c5c5d4977f39082348df2f831a4b72",
          "message": "Merge pull request #499 from blooop/release/0.24.0\n\nCut 0.24.0",
          "timestamp": "2026-08-28T21:42:40+01:00",
          "tree_id": "2c305a35efae3c1a8f43d89a764774d30f6e766e",
          "url": "https://github.com/blooop/devlaunch/commit/9da6255fb0c5c5d4977f39082348df2f831a4b72"
        },
        "date": 1787949940056,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.320641,
            "range": "± 0.097404",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.337379,
            "range": "± 0.008826",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000042,
            "range": "± 0.000015",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.672022,
            "range": "± 0.093586",
            "unit": "s",
            "extra": "runs=5/5 wall=1.674171s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.314201,
            "range": "± 0.080883",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 4.04984,
            "range": "± 1.016427",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.337035,
            "range": "± 0.007946",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.870499,
            "range": "± 0.096995",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.612211,
            "range": "± 1.024634",
            "unit": "s",
            "extra": "runs=5/5 wall=9.614353s v0.26.1, Linux-X64"
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
          "id": "669df0373ac7105d4385e57b9e4183082715d881",
          "message": "Merge pull request #495 from blooop/fix/restore-terminal-after-session\n\nPut the terminal back when a session's child is killed",
          "timestamp": "2026-08-28T22:08:19+01:00",
          "tree_id": "854bb444f6af723cf0a878f38c26a588782b2a49",
          "url": "https://github.com/blooop/devlaunch/commit/669df0373ac7105d4385e57b9e4183082715d881"
        },
        "date": 1787951468619,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.121391,
            "range": "± 0.118149",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.244583,
            "range": "± 0.16837",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000048,
            "range": "± 0.000004",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.455586,
            "range": "± 0.154497",
            "unit": "s",
            "extra": "runs=5/5 wall=1.457771s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.065173,
            "range": "± 0.047556",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.030957,
            "range": "± 0.443409",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.240289,
            "range": "± 0.013092",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.351594,
            "range": "± 0.282053",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 7.758243,
            "range": "± 0.428458",
            "unit": "s",
            "extra": "runs=5/5 wall=7.760633s v0.26.1, Linux-X64"
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
          "id": "c89f7d7257cbdf72a9f2d5f698fa785826e69b10",
          "message": "Merge pull request #513 from blooop/docs/431-honest-snapshot-sentence\n\ndocs: fix #431's false snapshot sentence; record the second-copies rule and review ritual in CLAUDE.md",
          "timestamp": "2026-08-29T18:06:34+01:00",
          "tree_id": "9a25f5864c0b7f40f2b48a1fd89acce5e23d580a",
          "url": "https://github.com/blooop/devlaunch/commit/c89f7d7257cbdf72a9f2d5f698fa785826e69b10"
        },
        "date": 1788023374603,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.379481,
            "range": "± 0.070616",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.359372,
            "range": "± 0.011017",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000043,
            "range": "± 0.000002",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.738504,
            "range": "± 0.065853",
            "unit": "s",
            "extra": "runs=5/5 wall=1.740753s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.361953,
            "range": "± 0.133037",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.251702,
            "range": "± 0.054333",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.333884,
            "range": "± 0.008663",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 4.031904,
            "range": "± 0.062765",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.004258,
            "range": "± 0.105152",
            "unit": "s",
            "extra": "runs=5/5 wall=9.006798s v0.26.1, Linux-X64"
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
          "id": "e8cc92c7504ce58dad8b4d49e3a566d66077aa8b",
          "message": "Merge pull request #512 from blooop/wayfinder/devlaunch-477\n\nfix: a bare's HEAD symref outlived the branch it named",
          "timestamp": "2026-08-29T18:41:32+01:00",
          "tree_id": "ced800eb5413da2b6e84d751c375161eb3fc2986",
          "url": "https://github.com/blooop/devlaunch/commit/e8cc92c7504ce58dad8b4d49e3a566d66077aa8b"
        },
        "date": 1788025475928,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.358304,
            "range": "± 0.062879",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.353136,
            "range": "± 0.016147",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000042,
            "range": "± 0.000001",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.715152,
            "range": "± 0.064246",
            "unit": "s",
            "extra": "runs=5/5 wall=1.717229s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.340582,
            "range": "± 0.072352",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.220905,
            "range": "± 0.86481",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.37098,
            "range": "± 0.027457",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.872053,
            "range": "± 0.032361",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.803431,
            "range": "± 0.886078",
            "unit": "s",
            "extra": "runs=5/5 wall=8.805448s v0.26.1, Linux-X64"
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
          "id": "bf64dd6c9815b4c55b5e851023784fef9e19c839",
          "message": "Merge pull request #500 from blooop/release/0.25.0\n\nCut 0.25.0",
          "timestamp": "2026-08-29T20:08:16+01:00",
          "tree_id": "ae5b1c71bb292339749afeb98e63420d998257b5",
          "url": "https://github.com/blooop/devlaunch/commit/bf64dd6c9815b4c55b5e851023784fef9e19c839"
        },
        "date": 1788030681289,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.360638,
            "range": "± 0.153627",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.361489,
            "range": "± 0.093913",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000028,
            "range": "± 0.000002",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.754319,
            "range": "± 0.217027",
            "unit": "s",
            "extra": "runs=5/5 wall=1.756038s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.281041,
            "range": "± 0.206204",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.553309,
            "range": "± 0.643515",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.417776,
            "range": "± 0.057187",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.656963,
            "range": "± 0.17842",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 9.45587,
            "range": "± 0.764266",
            "unit": "s",
            "extra": "runs=5/5 wall=9.457762s v0.26.1, Linux-X64"
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
          "id": "f3e1acb85c1bf5a797e3d7e0ea0460c62a13efec",
          "message": "Merge pull request #516 from blooop/wayfinder/devlaunch-456\n\nKeep devlaunch's own copy of the volume names, and reclaim from it",
          "timestamp": "2026-08-29T20:38:39+01:00",
          "tree_id": "2c73407eae77d1e498ddae725ce940489ce6655c",
          "url": "https://github.com/blooop/devlaunch/commit/f3e1acb85c1bf5a797e3d7e0ea0460c62a13efec"
        },
        "date": 1788032479822,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.192483,
            "range": "± 0.09949",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.29244,
            "range": "± 0.003916",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000043,
            "range": "± 0.000006",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.483438,
            "range": "± 0.097739",
            "unit": "s",
            "extra": "runs=5/5 wall=1.485317s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.224059,
            "range": "± 0.063252",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.042835,
            "range": "± 0.588122",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.158794,
            "range": "± 0.00567",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.562103,
            "range": "± 0.082916",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.050691,
            "range": "± 0.594906",
            "unit": "s",
            "extra": "runs=5/5 wall=8.052982s v0.26.1, Linux-X64"
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
          "id": "f0da25ca383acbe0a92a0f879e125b85c1e0d64d",
          "message": "Merge pull request #511 from blooop/wayfinder/devlaunch-308\n\nSweep dangling citations to retired Python tests",
          "timestamp": "2026-08-29T20:45:00+01:00",
          "tree_id": "6dd2116c72a42a56d0fabe61c929531da9b00e0e",
          "url": "https://github.com/blooop/devlaunch/commit/f0da25ca383acbe0a92a0f879e125b85c1e0d64d"
        },
        "date": 1788032883510,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.348194,
            "range": "± 0.108774",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.353542,
            "range": "± 0.00453",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000045,
            "range": "± 0.000007",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.700162,
            "range": "± 0.106717",
            "unit": "s",
            "extra": "runs=5/5 wall=1.702287s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.377143,
            "range": "± 0.042351",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.213846,
            "range": "± 0.169152",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.331204,
            "range": "± 0.011806",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.941279,
            "range": "± 0.134764",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.86186,
            "range": "± 0.313601",
            "unit": "s",
            "extra": "runs=5/5 wall=8.864228s v0.26.1, Linux-X64"
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
          "id": "af9f5a8fb2860afd35a63dbc5cc088d8c37c6d8d",
          "message": "Merge pull request #519 from blooop/wayfinder/devlaunch-508\n\nPersist the sweep's notices in the record, and let --ls read them",
          "timestamp": "2026-08-29T20:49:28+01:00",
          "tree_id": "8ec6ef5f0bbde4337f04ebf297c9d0703f8a45fc",
          "url": "https://github.com/blooop/devlaunch/commit/af9f5a8fb2860afd35a63dbc5cc088d8c37c6d8d"
        },
        "date": 1788033135012,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "warm / attach",
            "value": 1.234286,
            "range": "± 0.060385",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / devpod-up",
            "value": 0.304043,
            "range": "± 0.012043",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / host-prep",
            "value": 0.000047,
            "range": "± 0.000015",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "warm / total",
            "value": 1.539922,
            "range": "± 0.058703",
            "unit": "s",
            "extra": "runs=5/5 wall=1.542097s v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / attach",
            "value": 1.154355,
            "range": "± 0.110477",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / devpod-up",
            "value": 3.166043,
            "range": "± 0.981427",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / host-prep",
            "value": 0.179078,
            "range": "± 0.007534",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / tools",
            "value": 3.696503,
            "range": "± 0.182455",
            "unit": "s",
            "extra": "runs=5/5 v0.26.1, Linux-X64"
          },
          {
            "name": "cold-recreate / total",
            "value": 8.428128,
            "range": "± 1.009855",
            "unit": "s",
            "extra": "runs=5/5 wall=8.43048s v0.26.1, Linux-X64"
          }
        ]
      }
    ]
  }
}