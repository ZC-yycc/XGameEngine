#!/bin/bash
set -u
cd /d/Documents/Rust/XGameEngine
for N in 1 2 5 10 30 60 100 150; do
  cargo run -q -p xgame-editor --offline -- --headless --frames $N --dump-scene tmp/old-$N.ron projects/XGame > tmp/old-$N-report.json 2> tmp/old-$N.err
  echo "old $N exit=$?"
done
cd xgc
for N in 1 2 5 10 30 60 100 150; do
  cargo run -q -p xgc-cli --offline -- run-scripts ../tmp/showcase.json ../projects/XGame $N ../tmp/xgc-$N.json > /dev/null 2> ../tmp/xgc-$N.err
  echo "xgc $N exit=$?"
done
cd ..
for N in 1 2 5 10 30 60 100 150; do
  cd xgc
  cargo run -q -p xgc-cli --offline -- migrate ../tmp/old-$N.ron ../tmp/old-$N.migrated.json > /dev/null 2> ../tmp/mig-$N.err
  echo "migrate $N exit=$?"
  cd ..
done
echo ALL_DONE
