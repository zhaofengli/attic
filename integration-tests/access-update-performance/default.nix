{ pkgs, lib, ... }:
let
  python = pkgs.python3.withPackages (packages: [ packages.psycopg ]);
  benchmark = pkgs.writeScript "benchmark-access-updates.py" ''
    #!${python}/bin/python3
    import csv
    import statistics
    import time

    import psycopg

    BATCH_SIZES = (1, 16, 256, 1024, 4096)
    REPETITIONS = 10

    connection = psycopg.connect("dbname=attic user=root host=/run/postgresql")
    connection.autocommit = True

    with connection.cursor() as cursor:
        cursor.execute("CREATE TABLE object_access_benchmark (id BIGINT PRIMARY KEY, last_accessed_at TIMESTAMPTZ)")
        cursor.execute("INSERT INTO object_access_benchmark (id) SELECT generate_series(1, 4096)")

    writer = csv.writer(__import__("sys").stdout)
    writer.writerow(("batch_size", "strategy", "median_ms", "statements_per_batch"))

    for batch_size in BATCH_SIZES:
        object_ids = list(range(1, batch_size + 1))
        placeholders = ",".join(("%s",) * batch_size)
        timings = {"individual": [], "bulk": []}

        for _ in range(REPETITIONS + 1):
            for strategy in ("individual", "bulk"):
                with connection.cursor() as cursor:
                    cursor.execute("UPDATE object_access_benchmark SET last_accessed_at = NULL")
                    started_at = time.perf_counter_ns()
                    if strategy == "individual":
                        for object_id in object_ids:
                            cursor.execute(
                                "UPDATE object_access_benchmark SET last_accessed_at = NOW() WHERE id = %s",
                                (object_id,),
                            )
                    else:
                        cursor.execute(
                            f"UPDATE object_access_benchmark SET last_accessed_at = NOW() WHERE id IN ({placeholders})",
                            object_ids,
                        )
                    elapsed_ms = (time.perf_counter_ns() - started_at) / 1_000_000
                    cursor.execute(
                        "SELECT count(*) FROM object_access_benchmark WHERE last_accessed_at IS NOT NULL"
                    )
                    assert cursor.fetchone()[0] == batch_size
                timings[strategy].append(elapsed_ms)

        for strategy in ("individual", "bulk"):
            measured = timings[strategy][1:]
            writer.writerow((
                batch_size,
                strategy,
                f"{statistics.median(measured):.3f}",
                batch_size if strategy == "individual" else 1,
            ))
  '';
in {
  name = "access-update-performance";

  nodes.server = {
    services.postgresql = {
      enable = true;
      ensureDatabases = [ "attic" ];
      ensureUsers = [{
        name = "root";
        ensureClauses.superuser = true;
      }];
    };

    environment.systemPackages = [ python ];
  };

  testScript = ''
    import os
    from pathlib import Path

    start_all()
    server.wait_for_unit("postgresql.target")
    results = server.succeed("${benchmark}")
    print(results)

    output = Path(os.environ.get("out", os.getcwd()))
    output.mkdir(parents=True, exist_ok=True)
    (output / "access-update-benchmark.csv").write_text(results)
  '';
}
