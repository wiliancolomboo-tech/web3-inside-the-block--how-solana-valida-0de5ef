docker compose exec inside-the-block \
  sh -c 'sqlite3 /app/data/inside-the-block.db ".backup /tmp/backup.db"'

docker cp \
  "$(docker compose ps -q inside-the-block)":/tmp/backup.db \
  ./inside-the-block-backup.db