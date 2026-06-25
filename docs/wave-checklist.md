# Attic Wave Verification Checklist

Чеклист для разбора каждого Ray Tune wave, в котором 100 воркеров параллельно делают
env-pull через attic. Обязателен для каждого wave — даёт сопоставимые цифры между
запусками, отделяет реальные проблемы от шума.

**Контекст:**
- Ray Tune cluster, 100 подов; worker group зависит от запуска
  (`ray-kuberay-medium-worker-*`, `ray-kuberay-big-worker-*`, ...), кaждый делает
  `nix-store --realise` 7 store paths с substituter = `http://attic.attic.svc.cluster.local:9080/prefect`.
- Attic server: 4 пода, OSS (S3-compatible) + PostgreSQL serverless.
- Ray Tune `trial_init_timeout = 10 min` — env-pull, не успевший за 600 sec,
  получает `SIGTERM` (виден в логах nix_plugin как `cancelled, terminating pid=...`).
- Метрики attic в VictoriaMetrics: `atticd_http_requests_total`,
  `atticd_http_requests_duration_seconds_*`, `atticd_http_requests_pending`,
  `atticd_oss_request_duration_seconds_*`, `atticd_db_query_duration_seconds_*`,
  `atticd_metadata_cache_total{result="hit"|"miss"}` (добавлены нами через
  `metrics::histogram!`/`counter!`).
  **Важно:** oss/db duration оборачивают `req.send()` AWS SDK и `query_all` sea-orm,
  т.е. ВКЛЮЧАЮТ ожидание слота во внутренних пулах (sqlx pool: `database.max-connections`,
  default 25/под). Кратный рост этих латентностей при здоровых OSS/Postgres —
  очереди на пулах, а не деградация бэкендов.
- Логи в ClickHouse:
  - `analytics.ray_logs` — логи воркеров (включая `process='nix_plugin'`)
  - `analytics.kube_logs` — k8s pod logs всех неймспейсов, **включая `namespace_name='attic'`**
  - `analytics.kube_ray_logs` — только Ray-системные namespaces (`kube-ray`, `ray-test`, `ray`). НЕ для attic.

---

## Шаг 1. Окно wave

**`wave_start` задаёт пользователь** — это момент запуска `ray_cluster/submit.py`
(Prefect/CLI submit). НЕ "первый nix_plugin в логах" — между submit и первым
env-pull проходит Ray scheduling + image-pull (на холодной волне ~14 минут,
на тёплых нодах ~2 минуты).

**Таймзона:** ClickHouse и VictoriaMetrics — UTC, а таймстамп сабмита может быть
в локальной зоне (волна 2026-06-09: сабмит "09:42:59" = 08:42:59 UTC). Если в окне
пусто — не расширяй текстовые фильтры, а откалибруй зону: найди волну по объёму
логов (`GROUP BY toStartOfInterval(timestamp, INTERVAL 10 MINUTE)` за сутки),
затем сверь: `cancelled`-warning'и идут ровно через 600 sec после первого
`nix_plugin` своего пода — это однозначно привязывает волну к сабмиту.

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
| `avg_narpulls_per_pod` | ~700-800 (здоровые волны 2026-06-09: ~770) | сильно меньше — closure тоньше; сильно больше — closure вырос (волна 08:42 2026-06-09: ~1080; сверь с narpulls НЕотменённых подов — это фактический размер) или двойной счёт после ретраев |

**Ловушка:** для cancelled-подов `duration_sec` и `narpulls` включают ретрай после
SIGTERM — Ray перезапускает trial, nix_plugin продолжает логировать до конца волны,
поэтому p50 длительности может быть >600 sec. Фактический размер closure смотри
по выжившим (НЕотменённым) подам.

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
(там только Ray-namespaces). Колонка пода здесь — `pod_name` (в `ray_logs` — `pod`).

**Шаг 5.0 — сначала проверь, что логи вообще собираются:**

```sql
SELECT toStartOfHour(timestamp) AS h, count() AS cnt
FROM analytics.kube_logs
WHERE timestamp >= '<wave_start>' - INTERVAL 1 DAY
  AND timestamp < '<wave_end>' + INTERVAL 2 MINUTE
  AND namespace_name = 'attic'
GROUP BY h ORDER BY h
```

Нет строки за час волны = **сбор логов не работал** (реальный случай: gap с
~2026-06-08 13:41 до 2026-06-10/11, с единичным burst'ом 06-10 12:38), а не
"ошибок нет". Тогда шаг 5 пропускаем, server-side смотрим только по метрикам
(шаги 5b–6). Верхняя граница в запросе обязательна: `count()` за сутки без неё
захватывает логи после восстановления сбора и маскирует gap.

```sql
SELECT
  countIf(message ILIKE '%incompletemessage%'
          OR message ILIKE '%incomplete message%') AS oss_incomplete,
  countIf(message ILIKE '%slowdown%' OR message ILIKE '%503 slow down%'
          OR message ILIKE '%TotalQpsLimitExceeded%' OR message ILIKE '%QpsLimitExceeded%'
          OR message ILIKE '%TrafficRateLimitExceeded%'
          OR message ILIKE '%qos-delay%') AS oss_throttle,
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
- `oss_throttle` — **0**. Любое — OSS троттлит нас. Внимание: нативный Aliyun OSS
  НЕ отдаёт код "SlowDown" — троттлинг это 503 `TotalQpsLimitExceeded` /
  `*TrafficRateLimitExceeded`, 429 `QpsLimitExceeded` и заголовок
  `x-oss-qos-delay-time` (замедление без ошибки).
- `conn_closed` — единицы (idle timeout от RDS Proxy). Тревога: десятки за wave.
- `conn_err`, `timeout_cnt`, `db_pool_timeout` — 0 в норме.

**Замечание:** под `RUST_LOG=debug` attic генерирует **миллионы строк за wave** (~4M
за 15 мин). Запросы могут быть тяжёлыми — добавляй `LIMIT` если нужны примеры строк.

---

## Шаг 5b. Cross-check OSS / HTTP / DB (counts)

Превратить unix-таймштамп `wave_end + 3min` в число (Python: `int(datetime(...,
tzinfo=timezone.utc).timestamp())`), подставить в `@`. Окно `[Xm]` подобрать так,
чтобы накрыть весь env-pull: `X >= (wave_end - wave_start) + 3 мин` якорного
сдвига, обычно 15-20 мин. Для волны 2026-06-09 окно `[15m]` срезало бы первые
~3 минуты env-pull — поэтому в примерах ниже `[20m]`.

```promql
# OSS requests by op/status
sum by (op, status) (
  increase(atticd_oss_request_duration_seconds_count{namespace="attic"}[20m] @ <wave_end_unix+180>)
)

# HTTP requests total
sum (increase(atticd_http_requests_total{namespace="attic"}[20m] @ <wave_end_unix+180>))

# DB queries by status
sum by (status) (
  increase(atticd_db_query_duration_seconds_count{namespace="attic"}[20m] @ <wave_end_unix+180>)
)
```

**Derived ratios** (NAR_pulls берём из Шага 3 — `sum(narpulls)`):

| Ratio | Норма | Тревога | Что значит |
|---|---|---|---|
| `HTTP req / NAR_pull` | ~2.0 (narinfo + NAR на путь; здоровые волны 2026-06-09: 2.02-2.03) | >2.5 или <2.0 | >2.5 — retries или N+1 на `/get-missing-paths`; <2.0 — narpulls раздуты ретраями cancelled-подов (волна 08:42: 1.89) |
| `OSS GET / NAR_pull` (chunks/NAR) | цель 30-80; **фактический baseline ~155** | рост над baseline (2026-06-09: 243 — ретраи раздули) | stale chunking — старо-чанкнутые NAR'ы в closure; хронически >100, пока не сделан re-chunk |
| `OSS GET err / OSS GET ok` | 0 | >0.001 | OSS instability, AWS SDK retries; если err-серий нет вообще, запрос вернёт пусто — это 0 |
| `DB queries / NAR_pull` | ~2.5-3.0; после батчинга bump (фикс 2026-06) — ~2.0 | >5 | N+1 в attic, нужно профайлинг; bump теперь батчится раз в 30s, его count = число флашей, не NAR'ов |
| `OSS PUT` | первый wave: >0; следующие: 0 | внезапно >0 после первого | новые NAR'ы появляются — env изменился |

---

## Шаг 6. Метрики attic vs предыдущая волна (VM)

PromQL запросы (`@ <wave_end_unix+180>` ко всем):

```promql
# HTTP latency (метрика requests_duration, НЕ request_duration)
histogram_quantile(0.95, sum by (le) (rate(atticd_http_requests_duration_seconds_bucket{namespace="attic"}[20m] @ T)))
histogram_quantile(0.50, sum by (le) (rate(atticd_http_requests_duration_seconds_bucket{namespace="attic"}[20m] @ T)))

# OSS latency
sum(rate(atticd_oss_request_duration_seconds_sum{namespace="attic", op="get_object", status="ok"}[20m] @ T))
  / sum(rate(atticd_oss_request_duration_seconds_count{namespace="attic", op="get_object", status="ok"}[20m] @ T))
histogram_quantile(0.95, sum by (le) (rate(atticd_oss_request_duration_seconds_bucket{namespace="attic", op="get_object"}[20m] @ T)))

# DB latency
sum(rate(atticd_db_query_duration_seconds_sum{namespace="attic"}[20m] @ T))
  / sum(rate(atticd_db_query_duration_seconds_count{namespace="attic"}[20m] @ T))
histogram_quantile(0.95, sum by (le) (rate(atticd_db_query_duration_seconds_bucket{namespace="attic"}[20m] @ T)))

# RPS peak (мax по 30-сек окнам в течение волны)
max_over_time(sum(rate(atticd_http_requests_total{namespace="attic"}[30s]))[20m:30s] @ T)

# In-flight peak (метрика atticd_*, НЕ axum_*)
max_over_time(sum(atticd_http_requests_pending{namespace="attic"})[20m:30s] @ T)

# CPU / throttling attic-подов (проверка "ресурсы ок")
max_over_time(sum by (pod) (rate(container_cpu_usage_seconds_total{namespace="attic", container!="", container!="POD"}[1m]))[20m:30s] @ T)
sum by (pod) (increase(container_cpu_cfs_throttled_periods_total{namespace="attic"}[20m] @ T))
  / sum by (pod) (increase(container_cpu_cfs_periods_total{namespace="attic"}[20m] @ T))
```

**Интерпретация:**
- OSS/DB латентности включают pool-wait (см. Контекст). Синхронный кратный рост
  OSS и DB при CPU без throttling — насыщение по конкурентности, не деградация
  бэкендов; лечится репликами/пулами/prefetch, а не CPU.
- In-flight плато, почти не растущее с масштабом волны (~1450-1600 на 4 подах и
  при 30, и при 100 ray-подов; peak проблемной волны 2026-06-09 — 1798, всего
  +23%), — потолок ёмкости: спрос растёт, throughput стоит, латентность отдувается.

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
   - Пулы/конкурентность: OSS и DB растут синхронно в разы, CPU без throttling,
     in-flight на плато — пайплайн насыщен (волна 2026-06-09: OSS ×9, DB ×13,
     peak RPS всего +27% при ×4 спросе → 78/100 подов за cap)
   - Closure mix: `OSS GET/NAR` сильно выше baseline — старо-чанкнутые NAR'ы в env

   **Metadata cache (фикс 2026-06):** `find` теперь обслуживается из in-memory TTL-кэша
   на каждом поде. Hit-rate = `sum(rate(atticd_metadata_cache_total{result="hit"}[5m]))
   / sum(rate(atticd_metadata_cache_total[5m]))`. На волне с общим closure ожидается
   высокий hit-rate (после прогрева 4 подов); низкий hit-rate при общем closure =
   TTL короче волны или кэш выключен (`metadata-cache-ttl-seconds=0`). `find`-запросы
   к БД ≈ misses, т.е. DB/NAR падает ниже исторических 2.5-3.0.
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
| Таймстамп сабмита не в UTC | step 1: пустое окно | калибровка: волна по объёму логов за сутки + `cancelled = first_nix_plugin + 600s` |
| В `kube_logs` колонка `pod_name`, в `ray_logs` — `pod` | копипаста запросов между таблицами | `Code: 47 UNKNOWN_IDENTIFIER` — проверь имя колонки |
| Все счётчики шага 5 = 0 | step 5 | это может быть gap сбора логов, а не "нет ошибок" — сначала шаг 5.0 |
| `duration_sec` > 600 у cancelled-подов | step 3 | ретрай trial'а после SIGTERM продолжает логировать; не ошибка подсчёта |

---

## Связанные источники

- **Текущая конфигурация атика**: `deploy-cn-infra/deploy_pulumi/deploy_core/ali/attic/setup.py`
  (image tag, max-connections, num_prefetch указаны там и в коде attic).
- **Патчи на attic**: ветка `defy/deploy` (на базе upstream `origin/pr311`) в
  `/Users/izolin/attic` (drain-fix в `server/src/api/v1/upload_path.rs`,
  num_prefetch=16 в `merge_chunks` — захардкожен в `server/src/api/binary_cache.rs`
  (TODO: сделать конфигурируемым), метрики).
- **DB pool**: `server/src/config.rs`, `database.max-connections` (default 25/под);
  фактически в setup.py — **400/под**. Метрики db/oss включают pool-wait.
- **Лимиты бэкендов** (проверено по докам Aliyun, 2026-06): Postgres —
  `pg.n2.serverless.1c` serverless_basic (single-node), RCU 0.5-8 (потолок серии
  ~14, дальше только смена серии), `max_connections` **фиксирован 2400** и от RCU
  не зависит — суммарный sqlx-пул (max-connections × поды) держать <= ~2000.
  OSS — дефолтный лимит аккаунта 10k QPS (фактически тянет 30k+ GET/s на
  хэш-ключах за счёт авто-партиционирования; гарантий нет, квота — через сапорт),
  bandwidth SG: 100 Gbit/s total download, 2k QPS / 10 Gbit/s на один объект.
- **Пример разбора**: волна 2026-06-09 08:42:59 UTC (сабмит "09:42:59" в логе) —
  78/100 cancelled; диагноз: насыщение по конкурентности (in-flight плато ~1500,
  OSS ×9, DB ×13 от pool-wait) + chunk-амплификация ~155 GET/NAR + closure +40%
  (770→1080 путей/под).
- **Метрики attic**: `attic-server` шкрапится через `VMServiceScrape` в namespace=attic;
  селектор `monitoring: attic-metrics`.
- **Ray Tune trial timeout**: 600 sec (`trial_init_timeout`) — application-level
  настройка Ray Tune (конфиг волны в `ray_cluster/submit.py`), НЕ KubeRay chart
  (тот конфигурирует кластер/поды, а не таймауты trial'ов).
