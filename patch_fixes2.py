import io
import re
import sys


def load(path):
    with io.open(path, "r", encoding="utf-8", newline="") as handle:
        raw = handle.read()
    return ("\r\n" in raw), raw.replace("\r\n", "\n")


def save(path, text, crlf):
    if crlf:
        text = text.replace("\n", "\r\n")
    with io.open(path, "w", encoding="utf-8", newline="") as handle:
        handle.write(text)


def patch(path, old, new):
    crlf, text = load(path)
    if old not in text:
        sys.exit("anchor missing in " + path + ": " + old[:60])
    save(path, text.replace(old, new, 1), crlf)


fallback = r"crates\axon-session\src\infrastructure\sqlite_runtime_store\projection_read_fallback.rs"

# 1. 去掉「已计入轮次」去重：水位已是批末序号，落后窗口恰好等于未投影的事件，
#    再按 turn_activity 去重会在「读数已写、指标写失败」时永久少算。
patch(
    fallback,
    '''    let turns: Vec<(String, TurnMetrics)> = records.iter().filter_map(turn_metrics_of).collect();
    if turns.is_empty() {
        return Some(MetricsDelta::default());
    }
    let owners: Vec<&str> = turns.iter().map(|(owner, _)| owner.as_str()).collect();
    let Some(counted) = counted_interactions(store, session_id, &owners) else {
        report_failed(session_id, "会话累计指标", "已计入轮次的查证失败");
        return None;
    };
    let mut delta = MetricsDelta::default();
    for (owner, metrics) in &turns {
        // 已经在投影里计过数的轮次不再累加：水位是批首序号，最后一批通常早已投影完。
        if counted.contains(owner) {
            continue;
        }
        delta.add(metrics);
    }
    Some(delta)''',
    '''    /*
     * 不按轮次去重：水位记的是「已投影到的最后一条序号」，这段区间恰好就是尚未投影的
     * 事件，区间里的每一条指标都还没进累计值，全部累加才对。按 `turn_activity` 去重会
     * 把「读数已写、指标写失败」的轮次跳过，变成永久少算。
     */
    let mut delta = MetricsDelta::default();
    for (_, metrics) in records.iter().filter_map(turn_metrics_of) {
        delta.add(&metrics);
    }
    Some(delta)''')

# 2. 删除 counted_interactions 函数
crlf, text = load(fallback)
start = text.find("/// 已经在投影里计过数的轮次。")
if start < 0:
    sys.exit("counted doc missing")
end_marker = "\n/// 累计指标回源"
end = text.find(end_marker, start)
if end < 0:
    sys.exit("counted function end missing")
text = text[:start] + text[end + 1 :]
save(fallback, text, crlf)

# 3. 重建保留作业计数，并补一个启动对账入口
rebuild = r"crates\axon-session\src\infrastructure\sqlite_runtime_store\projection_rebuild.rs"

patch(
    rebuild,
    '''        self.clear_session_projections(session_id)?;''',
    '''        /*
         * 作业计数由宿主按作业事件增量维护，日志里没有可重放的来源；重建前先读出来，
         * 重放后原样写回，否则会被清零。
         */
        let jobs = self.session_jobs_counts(session_id)?;
        self.clear_session_projections(session_id)?;''')

patch(
    rebuild,
    '''            after = last;
        }
    }''',
    '''            after = last;
        }
    }

    /// 写回重建前读到的作业计数；行不存在时跳过（没有轮次指标的会话本来就没有这一行）。
    fn restore_session_jobs(
        &self,
        session_id: &DaemonSessionId,
        jobs: Option<(i64, i64, i64, i64)>,
    ) -> Result<(), RuntimeStoreError> {
        let Some((started, completed, failed, running)) = jobs else {
            return Ok(());
        };
        self.with_read_connection(|connection| {
            connection.execute(
                "UPDATE session_metrics SET jobs_started=?1, jobs_completed=?2, \\
                    jobs_failed=?3, jobs_running=?4 WHERE session_id=?5",
                rusqlite::params![started, completed, failed, running, session_id.as_str()],
            )
        })?;
        Ok(())
    }

    /// 读当前作业计数；没有行时返回 `None`。
    fn session_jobs_counts(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<Option<(i64, i64, i64, i64)>, RuntimeStoreError> {
        use rusqlite::OptionalExtension as _;
        self.with_read_connection(|connection| {
            connection
                .query_row(
                    "SELECT jobs_started, jobs_completed, jobs_failed, jobs_running \\
                       FROM session_metrics WHERE session_id=?1",
                    [session_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
        })
    }

    /// Daemon 启动对账：水位超过事件水位的会话，日志必然被截断或整体替换过。
    ///
    /// 启动时是单写者窗口，重建不会与投影 worker 并发；返回重建的会话数。
    pub fn rebuild_rewound_projections(&self) -> Result<usize, RuntimeStoreError> {
        let sessions = self.with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT s.session_id FROM sessions AS s \\
                   JOIN projection_state AS p ON p.session_id = s.session_id \\
                  WHERE p.last_projected_seq > s.last_event_sequence",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<String>>>()
        })?;
        let mut rebuilt = 0usize;
        for raw in sessions {
            let Ok(session_id) = DaemonSessionId::new(raw) else {
                continue;
            };
            match self.rebuild_session_projections(&session_id) {
                Ok(_) => rebuilt += 1,
                Err(error) => axon_logger::warning(&format!(
                    "启动投影重建失败，保留原投影: session={} error={error}",
                    session_id.as_str()
                )),
            }
        }
        Ok(rebuilt)
    }''')

# 重建流程末尾调用写回：在 Ok(0) 早退与正常返回两处都需要，改为在 clear 后重放的 finally 语义
patch(
    rebuild,
    '''            if records.is_empty() {
                return Ok(replayed);
            }''',
    '''            if records.is_empty() {
                self.restore_session_jobs(session_id, jobs)?;
                return Ok(replayed);
            }''')

print("patched")
