# Attic Wave Verification Checklist

Чеклист для разбора каждого Ray Tune wave, в котором 100 воркеров параллельно делают
env-pull через attic. Обязателен для каждого wave — даёт сопоставимые цифры между
запусками, отделяет реальные проблемы от шума.

**Контекст:**
- Ray Tune cluster (`ray-kuberay-medium-worker-*`), 100 подов, кaждый делает
  `nix-store --realise` 7 store paths с substituter = `http://attic.attic.svc.cluster.local:9080/prefect`.
- Attic server: 4 пода, OSS (S3-compatible) + PostgreSQL serverless.
- Ray Tune `trial_init_timeout = 10 min` — env-pull, не успевший за 600 sec,
  получает `SIGTERM` (виден в логах nix_plugin как `cancelled, terminating pid=...`).
- Метрики attic в VictoriaMetrics: `atticd_http_*`, `atticd_oss_*`, `atticd_db_*`
  (добавлены нами через `metrics::histogram!`).
- Логи в ClickHouse:
  - `analytics.ray_logs` — логи воркеров (включая `process='nix_plugin'`)
  - `analytics.kube_logs` — k8s pod logs всех неймспейсов, **включая `namespace_name='attic'`**
  - `analytics.kube_ray_logs` — только Ray-системные namespaces (`kube-ray`, `ray-test`, `ray`). НЕ для attic.

---

## Шаг 1. Окно wave

**`wave_start` задаёт пользователь** — это момент запуска `ray_cluster/submit.py`
(Prefect/CLI submit). НЕ "первый nix_plugin в логах" — между submit и первым
env-pull проходит Ray scheduling + image-pull (в недавней волне ~14 минут).

**`wave_end` — авто**, последний таймстамп `nix_plugin` по всем подам:

```sql
SELECT max(timestamp) AS wave_end
FROM analytics.ray_logs
WHERE timestamp >= '<wave_start>'
  AND timestamp < '<wave_start> + 30min'
  AND process = 'nix_plugin'
```

Дальше во всех запросах: `timestamp BETWEEN '<wave_start>' AND '<wave_end> + 2min'`.

---

## Шаг 2. Подсчёт воркеров и time-to-start

Максимум воркеров — 100, но они поднимаются с разбросом (image-pull, Ray scheduling).
Нужно отделить тех кто дошёл до env-pull от тех кто застрял раньше.

```sql
WITH per_pod AS (
  SELECT
    pod,
    min(timestamp) AS first_nix_plugin
  FROM analytics.ray_logs
  WHERE timestamp BETWEEN '<wave_start>' AND '<wave_end> + 2min'
    AND process = 'nix_plugin'
  GROUP BY pod
)
SELECT
  count() AS pods_with_nix_plugin,
  100 - count() AS pods_missing,                                              -- не дошли до env-pull вообще
  round(avg(dateDiff('second', toDateTime64('<wave_start>', 3), first_nix_plugin))) AS avg_ttn_sec,
  quantile(0.5)(dateDiff('second', toDateTime64('<wave_start>', 3), first_nix_plugin)) AS p50_ttn,
  quantile(0.95)(dateDiff('second', toDateTime64('<wave_start>', 3), first_nix_plugin)) AS p95_ttn,
  max(dateDiff('second', toDateTime64('<wave_start>', 3), first_nix_plugin)) AS max_ttn
FROM per_pod
```

**На что смотреть:**

| Метрика | Норма | Тревога |
|---|---|---|
| `pods_missing` | 0 | >0 = поды не запустились (image pull fail / OOM / cluster scheduling) |
| `p50_ttn` (time-to-nix_plugin) | <120 sec | >300 sec = долгий image pull / Ray scheduling узкое место |
| `p95_ttn` | <300 sec | >600 sec = эти поды съели большую часть Ray cap до env-pull |
| `max_ttn` | <p95 + 60 sec | большой разброс = неравномерное scheduling |

Если `pods_missing > 0` — разбирать через `analytics.kube_logs` namespace=`kube-ray` (или другой Ray namespace) и `kubectl describe pod` чтобы найти причину.

---

## Шаг 3. env-pull duration per pod — главная метрика

```sql
WITH per_pod AS (
  SELECT
    pod,
    dateDiff('second', min(timestamp), max(timestamp)) AS duration_sec,
    countIf(level = 'WARNING' AND message ILIKE '%cancelled%') AS cancelled,
    countIf(message ILIKE '%copying path%') AS narpulls
  FROM analytics.ray_logs
  WHERE timestamp BETWEEN '<wave_start>' AND '<wave_end> + 5min'
    AND process = 'nix_plugin'
  GROUP BY pod
)
SELECT
  count() AS pods,
  countIf(cancelled > 0) AS cancelled_pods,        -- упёрлись в Ray 10-min cap
  countIf(duration_sec >= 595) AS near_cap_pods,   -- ≥9:55, подозрение на cap
  round(avg(duration_sec)) AS avg_sec,
  quantile(0.5)(duration_sec) AS p50,
  quantile(0.9)(duration_sec) AS p90,
  quantile(0.95)(duration_sec) AS p95,
  quantile(0.99)(duration_sec) AS p99,
  max(duration_sec) AS max_sec,
  min(duration_sec) AS min_sec,
  sum(narpulls) AS total_narpulls,
  round(avg(narpulls)) AS avg_narpulls_per_pod
FROM per_pod
```

**На что смотреть:**

| Поле | Норма | Тревога |
|---|---|---|
| `cancelled_pods` | 0 | >0 = Ray прибил env-pull по cap'у |
| `near_cap_pods` | <5 | >10 = многие на грани, в след. wave упрутся |
| `p50` | <7 min | >9 min = деградация |
| `p95` | <9 min | >9:30 = почти cap |
| `min` outlier (<60 sec) | информативно | показывает что node-level Nix store hot для части подов |
| `avg_narpulls_per_pod` | ~700-800 | если сильно меньше — closure тоньше; если сильно больше — fragmentation в БД |

---

## Шаг 4. Ошибки nix_plugin (substituter-side)

```sql
SELECT
  countIf(message ILIKE '%timeout%' OR message ILIKE '%timed out%') AS timeout_cnt,
  countIf(message ILIKE '%deadline%') AS deadline_cnt,
  countIf(message ILIKE '%refused%' OR message ILIKE '%reset by peer%'
          OR message ILIKE '%broken pipe%') AS netfail_cnt,
  countIf(message ILIKE '%unexpected EOF%'
          OR message ILIKE '%unexpected end of file%') AS eof_cnt,
  countIf(message ILIKE '%retry%' OR message ILIKE '%retrying%') AS retry_cnt,
  countIf(level = 'WARNING' AND message NOT ILIKE '%cancelled%') AS real_warnings
FROM analytics.ray_logs
WHERE timestamp BETWEEN '<wave_start>' AND '<wave_end>'
  AND process = 'nix_plugin'
```

**Важно:**
- НЕ использовать `message ILIKE '%error%'` без других фильтров — путь
  `libgpg-error-1.51-dev` даст 100+ false positives.
- НЕ запрашивать без `process = 'nix_plugin'` — попадут `core_worker` / `raylet`
  Ray-internal "worker exited because it was idle (timeout: 10000ms)" сообщения,
  которые не имеют отношения к attic.

---

## Шаг 5. Логи attic (server-side)

Attic-логи живут в `analytics.kube_logs` (общая таблица), **НЕ в `kube_ray_logs`**
(там только Ray-namespaces).

```sql
SELECT
  countIf(message ILIKE '%incompletemessage%'
          OR message ILIKE '%incomplete message%') AS oss_incomplete,
  countIf(message ILIKE '%slowdown%' OR message ILIKE '%503 slow down%') AS oss_slowdown,
  countIf(message ILIKE '%connection%closed%'
          OR message ILIKE '%broken pipe%') AS conn_closed,
  countIf(message ILIKE '%connection refused%'
          OR message ILIKE '%connection reset%') AS conn_err,
  countIf(message ILIKE '%timeout%' OR message ILIKE '%timed out%') AS timeout_cnt,
  countIf(message ILIKE '%pool timed out%'
          OR message ILIKE '%acquire connection%') AS db_pool_timeout,
  countIf(message ILIKE '%ERROR%' OR message ILIKE 'error%') AS error_lines,
  count() AS total
FROM analytics.kube_logs
WHERE timestamp BETWEEN '<wave_start>' AND '<wave_end> + 2min'
  AND namespace_name = 'attic'
```

**Ожидаемое (норма для здоровой волны):**
- `oss_incomplete` — единицы (OSS изредка отдаёт partial). Тревога: десятки+.
- `oss_slowdown` — **0**. Любое — OSS троттлит нас.
- `conn_closed` — единицы (idle timeout от RDS Proxy). Тревога: десятки за wave.
- `conn_err`, `timeout_cnt`, `db_pool_timeout` — 0 в норме.

**Замечание:** под `RUST_LOG=debug` attic генерирует **миллионы строк за wave** (~4M
за 15 мин). Запросы могут быть тяжёлыми — добавляй `LIMIT` если нужны примеры строк.

---

## Шаг 5b. Cross-check OSS / HTTP / DB (counts)

Превратить unix-таймштамп `wave_end + 3min` в число (Python: `int(datetime(...,
tzinfo=timezone.utc).timestamp())`), подставить в `@`. Окно `[Xm]` подобрать так,
чтобы накрыть весь env-pull (10-15 мин обычно).

```promql
# OSS requests by op/status
sum by (op, status) (
  increase(atticd_oss_request_duration_seconds_count{namespace="attic"}[15m] @ <wave_end_unix+180>)
)

# HTTP requests total
sum (increase(atticd_http_requests_total{namespace="attic"}[15m] @ <wave_end_unix+180>))

# DB queries by status
sum by (status) (
  increase(atticd_db_query_duration_seconds_count{namespace="attic"}[15m] @ <wave_end_unix+180>)
)
```

**Derived ratios** (NAR_pulls берём из Шага 3 — `sum(narpulls)`):

| Ratio | Норма | Тревога | Что значит |
|---|---|---|---|
| `HTTP req / NAR_pull` | ~1.0 | >1.3 | retries или N+1 на `/get-missing-paths` |
| `OSS GET / NAR_pull` (chunks/NAR) | 30-80 | >100 | stale chunking — старо-чанкнутые NAR'ы в closure |
| `OSS GET err / OSS GET ok` | 0 | >0.001 | OSS instability, AWS SDK retries |
| `DB queries / NAR_pull` | ~2.5 | >5 | N+1 в attic, нужно профайлинг |
| `OSS PUT` | первый wave: >0; следующие: 0 | внезапно >0 после первого | новые NAR'ы появляются — env изменился |

---

## Шаг 6. Метрики attic vs предыдущая волна (VM)

PromQL запросы (`@ <wave_end_unix+180>` ко всем):

```promql
# HTTP latency
histogram_quantile(0.95, sum by (le) (rate(atticd_http_request_duration_seconds_bucket{namespace="attic"}[15m] @ T)))
histogram_quantile(0.50, sum by (le) (rate(atticd_http_request_duration_seconds_bucket{namespace="attic"}[15m] @ T)))

# OSS latency
sum(rate(atticd_oss_request_duration_seconds_sum{namespace="attic", op="get_object"}[15m] @ T))
  / sum(rate(atticd_oss_request_duration_seconds_count{namespace="attic", op="get_object", status="ok"}[15m] @ T))
histogram_quantile(0.95, sum by (le) (rate(atticd_oss_request_duration_seconds_bucket{namespace="attic", op="get_object"}[15m] @ T)))

# DB latency
sum(rate(atticd_db_query_duration_seconds_sum{namespace="attic"}[15m] @ T))
  / sum(rate(atticd_db_query_duration_seconds_count{namespace="attic"}[15m] @ T))
histogram_quantile(0.95, sum by (le) (rate(atticd_db_query_duration_seconds_bucket{namespace="attic"}[15m] @ T)))

# RPS peak (мax по 30-сек окнам в течение волны)
max_over_time(sum(rate(atticd_http_requests_total{namespace="attic"}[30s]))[15m:30s] @ T)

# In-flight peak
max_over_time(sum(axum_http_requests_pending{namespace="attic"})[15m:30s] @ T)
```

Сравнительная таблица с предыдущей волной:

| Метрика | prev | current | Δ |
|---|---|---|---|
| env-pull p50 (nix_plugin) | | | |
| env-pull p95 | | | |
| cancelled_pods | | | |
| HTTP p95 (attic) | | | |
| OSS get_object avg | | | |
| OSS get_object p95 | | | |
| DB query avg | | | |
| DB query p95 | | | |
| Peak RPS | | | |
| In-flight peak | | | |
| OSS GET total | | | |
| OSS GET/NAR | | | |
| HTTP/NAR | | | |
| DB/NAR | | | |

---

## Шаг 7. Вывод

Ответить на 4 вопроса:

1. **Сколько подов реально дошли до trial?** — `100 - cancelled_pods - failed_pods`.
2. **env-pull стал быстрее или медленнее?** — Δ p50, Δ p95 в минутах vs предыдущая волна.
3. **Какая подсистема bottleneck?**
   - DB: `Δ DB p95` доминирует
   - OSS: `Δ OSS p95` доминирует и `OSS GET/NAR` растёт
   - attic CPU/HTTP: `HTTP p95` растёт при стабильных DB/OSS
   - Closure mix: `OSS GET/NAR > 100` — старо-чанкнутые NAR'ы в env
4. **Что меняем в следующей итерации?** — конкретный эксперимент: параметр /
   код / SQL UPDATE.

---

## Известные источники шума (false positives) и ловушки

| Шум | Где возникает | Как фильтровать |
|---|---|---|
| `Worker exited because it was idle for a long time (timeout: 10000ms)` | `ray_logs` без фильтра по `process` | добавить `process = 'nix_plugin'` |
| Пути `libgpg-error-*` совпадают с `%error%` | `nix_plugin` поиск ошибок | не использовать `%error%`, использовать конкретные паттерны |
| `Event stats` строки с "timeout"/"deadline" в названиях метрик | `ray_logs` уровень DEBUG/TRACE | фильтр `level IN ('WARNING', 'ERROR')` |
| Aliyun RDS LB IPs `10.49.59.41/42` в `pg_stat_activity` маскируют per-pod | postgres диагностика | помнить про LB; считать по `count(*) FILTER (WHERE state='idle')` суммарно |
| Attic-логи **НЕ в `kube_ray_logs`** | step 5 (attic server-side errors) | использовать `analytics.kube_logs` с `namespace_name='attic'` |
| Wave_start = первый nix_plugin (НЕВЕРНО) | подсчёт `pods_with_nix_plugin` | `wave_start` = момент `ray_cluster/submit.py`, задаёт пользователь |
| Pods count = 100 (НЕВЕРНО для свежей волны) | step 2 | поды поднимаются с разбросом; считать `pods_with_nix_plugin` + `pods_missing` |

---

## Связанные источники

- **Текущая конфигурация атика**: `deploy-cn-infra/deploy_pulumi/deploy_core/ali/attic/setup.py`
  (image tag, max-connections, num_prefetch указаны там и в коде attic).
- **Патчи на attic**: ветка `pr311-defy-*` в `/Users/izolin/attic` (drain-fix в
  `server/src/api/v1/upload_path.rs`, num_prefetch=16 в `merge_chunks`, метрики).
- **Метрики attic**: `attic-server` шкрапится через `VMServiceScrape` в namespace=attic;
  селектор `monitoring: attic-metrics`.
- **Ray Tune trial timeout**: 600 sec, source — Ray cluster config (`KubeRay` chart).
