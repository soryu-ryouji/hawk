using Microsoft.Data.Sqlite;

namespace Hawk.Server.Core;

/// <summary>
/// 元数据的本地 SQLite 派生缓存（库外缓存目录 index.db，与 thumbnails/colors 同目录）。
///
/// 定位：.hawk/metadata/*.toml 是唯一权威数据源并参与网盘同步；本库只是本机加速器——
/// 启动时据此一次顺序读注水内存索引（避免全量解析 TOML），MetadataStore 写入时先 TOML
/// 后本库。对账只进不出：后台对账仅把 TOML 的变更搬进本库，绝不反向生成 TOML。
///
/// 缓存可随时整体删除，重启后从 TOML 全量重建（一次性慢启动）；删除缓存不影响数据安全。
/// 任何故障（打不开/写失败/读失败）只退化性能：MetadataStore 回到纯 TOML 行为，绝不影响权威数据。
/// </summary>
public sealed class IndexDb : IDisposable
{
    private const int SchemaVersion = 1;
    private const string HydratedKey = "hydrated";

    private readonly object _gate = new();
    private readonly ILogger<IndexDb> _logger;
    private SqliteConnection? _conn;

    /// <summary>写失败后熔断：后续操作一律短路，等价于缓存不存在（纯 TOML 行为）</summary>
    private bool _poisoned;

    /// <summary>缓存是否已完成至少一次全量注水。false 时内容不可信，必须由 TOML 全量重建</summary>
    public bool IsHydrated { get; private set; }

    /// <summary>缓存可用（打开成功且未熔断）。不可用时全部方法安全短路</summary>
    public bool Enabled
    {
        get
        {
            lock (_gate)
            {
                return _conn is not null && !_poisoned;
            }
        }
    }

    public IndexDb(LibraryPaths paths, ILogger<IndexDb> logger)
    {
        _logger = logger;
        try
        {
            // journal_mode=DELETE（默认回滚日志）：提交直接写进主库文件，不产生 -wal/-shm 伴生文件。
            // 不选 WAL：本进程全部 db 访问都在索引流水线单线程（启动注水/写穿/对账读），
            // 读写并发优势用不上；DELETE 让「写入 = 主库文件」的语义对使用者一目了然。
            // Pooling=false：连接关闭即真正释放，文件锁不拖过 Dispose。
            var builder = new SqliteConnectionStringBuilder { DataSource = paths.IndexDbFile, Pooling = false };
            _conn = new SqliteConnection(builder.ConnectionString);
            _conn.Open();
            Execute("PRAGMA journal_mode=DELETE");
            Execute("PRAGMA busy_timeout=5000");
            Execute("PRAGMA synchronous=NORMAL");
            InitSchema();
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "元数据缓存打开失败，退化为纯 TOML 模式（仅影响启动与查询速度）");
            lock (_gate)
            {
                _conn?.Dispose();
                _conn = null;
            }
        }
    }

    /// <summary>
    /// 全量注水（仅启动时、缓存缺失/重建后调用一次）：事务内清空并写入全部条目，最后置 hydrated 标记。
    /// 空库也是合法完成态（新素材库），同样置标记。失败则保持未注水，下次启动重试。
    /// </summary>
    public void Hydrate(IReadOnlyCollection<(string Hash, ItemMetadata Meta, long SourceMtime)> entries)
    {
        if (!Enabled)
        {
            return;
        }

        try
        {
            lock (_gate)
            {
                using var tx = _conn!.BeginTransaction();
                Execute("DELETE FROM paths", tx);
                Execute("DELETE FROM tags", tx);
                Execute("DELETE FROM categories", tx);
                Execute("DELETE FROM items", tx);
                foreach (var (hash, meta, mtime) in entries)
                {
                    InsertItem(hash, meta, mtime, tx);
                }

                tx.Commit();
            }

            WriteMeta(HydratedKey, "1");
            IsHydrated = true;
            _logger.LogInformation("元数据缓存已注水 {Count} 条", entries.Count);
        }
        catch (Exception ex)
        {
            Poison(ex, "元数据缓存注水失败，保持未注水状态");
        }
    }

    /// <summary>全量读取（启动注水内存用）。缓存不可用或损坏时抛异常，调用方回退 TOML 全量解析</summary>
    public List<(string Hash, ItemMetadata Meta, long SourceMtime)> LoadAll()
    {
        lock (_gate)
        {
            EnsureUsable();
            var items = new Dictionary<string, (ItemMetadata Meta, long SourceMtime)>(StringComparer.Ordinal);
            using (var cmd = _conn!.CreateCommand())
            {
                cmd.CommandText = "SELECT hash, url, star, annotation, source_mtime, width, height, palette, palette_version FROM items";
                using var reader = cmd.ExecuteReader();
                var hashOrd = reader.GetOrdinal("hash");
                var urlOrd = reader.GetOrdinal("url");
                var starOrd = reader.GetOrdinal("star");
                var annOrd = reader.GetOrdinal("annotation");
                var mtimeOrd = reader.GetOrdinal("source_mtime");
                var widthOrd = reader.GetOrdinal("width");
                var heightOrd = reader.GetOrdinal("height");
                var paletteOrd = reader.GetOrdinal("palette");
                var paletteVersionOrd = reader.GetOrdinal("palette_version");
                while (reader.Read())
                {
                    var meta = new ItemMetadata
                    {
                        Url = reader.IsDBNull(urlOrd) ? null : reader.GetString(urlOrd),
                        Star = (int)reader.GetInt64(starOrd),
                        Annotation = reader.IsDBNull(annOrd) ? null : reader.GetString(annOrd),
                        Width = (int)reader.GetInt64(widthOrd),
                        Height = (int)reader.GetInt64(heightOrd),
                        Palette = ParsePaletteJson(reader.IsDBNull(paletteOrd) ? null : reader.GetString(paletteOrd)),
                        PaletteVersion = (int)reader.GetInt64(paletteVersionOrd),
                    };
                    items[reader.GetString(hashOrd)] = (meta, reader.GetInt64(mtimeOrd));
                }
            }

            using (var cmd = _conn.CreateCommand())
            {
                cmd.CommandText = "SELECT hash, path, size, mtime FROM paths";
                using var reader = cmd.ExecuteReader();
                var hashOrd = reader.GetOrdinal("hash");
                var pathOrd = reader.GetOrdinal("path");
                var sizeOrd = reader.GetOrdinal("size");
                var mtimeOrd = reader.GetOrdinal("mtime");
                while (reader.Read())
                {
                    if (items.TryGetValue(reader.GetString(hashOrd), out var entry))
                    {
                        entry.Meta.Paths.Add(new PathEntry
                        {
                            Path = reader.GetString(pathOrd),
                            Size = reader.GetInt64(sizeOrd),
                            ModificationTime = reader.GetInt64(mtimeOrd),
                        });
                    }
                }
            }

            LoadChildRows("SELECT hash, tag FROM tags", "tag", items, (meta, value) => meta.Tags.Add(value));
            LoadChildRows("SELECT hash, category FROM categories", "category", items, (meta, value) => meta.Categories.Add(value));

            var result = new List<(string, ItemMetadata, long)>(items.Count);
            foreach (var (hash, entry) in items)
            {
                result.Add((hash, entry.Meta, entry.SourceMtime));
            }

            return result;
        }
    }

    /// <summary>写穿：镜像一条元数据及其 TOML 源文件 mtime（对账比对依据）。失败熔断，不影响权威数据</summary>
    public void Save(string hash, ItemMetadata meta, long sourceMtime)
    {
        if (!Enabled)
        {
            return;
        }

        try
        {
            lock (_gate)
            {
                using var tx = _conn!.BeginTransaction();
                Execute(
                    "INSERT INTO items (hash, url, star, annotation, source_mtime, width, height, palette, palette_version) " +
                    "VALUES ($h, $u, $s, $a, $m, $w, $t, $p, $v) " +
                    "ON CONFLICT(hash) DO UPDATE SET url=$u, star=$s, annotation=$a, source_mtime=$m, width=$w, height=$t, palette=$p, palette_version=$v",
                    tx,
                    ("$h", hash), ("$u", meta.Url ?? (object)DBNull.Value), ("$s", meta.Star),
                    ("$a", meta.Annotation ?? (object)DBNull.Value), ("$m", sourceMtime),
                    ("$w", meta.Width), ("$t", meta.Height), ("$p", (object?)PaletteJson(meta.Palette) ?? DBNull.Value), ("$v", meta.PaletteVersion));
                DeleteChildRows(hash, tx);
                InsertChildRows(hash, meta, tx);
                tx.Commit();
            }
        }
        catch (Exception ex)
        {
            Poison(ex, $"元数据缓存写入失败 {hash}");
        }
    }

    /// <summary>删除一条镜像。失败熔断，不影响权威数据</summary>
    public void Delete(string hash)
    {
        if (!Enabled)
        {
            return;
        }

        try
        {
            lock (_gate)
            {
                using var tx = _conn!.BeginTransaction();
                Execute("DELETE FROM items WHERE hash=$h", tx, ("$h", hash));
                DeleteChildRows(hash, tx);
                tx.Commit();
            }
        }
        catch (Exception ex)
        {
            Poison(ex, $"元数据缓存删除失败 {hash}");
        }
    }

    /// <summary>全量文件夹快照（增量扫描的对比基准）。缓存不可用时返回空表（首轮 = 全量深入）</summary>
    public IReadOnlyDictionary<string, (long Mtime, int Entries)> LoadFolderSnapshots()
    {
        if (!Enabled)
        {
            return new Dictionary<string, (long, int)>(StringComparer.Ordinal);
        }

        lock (_gate)
        {
            EnsureUsable();
            var result = new Dictionary<string, (long, int)>(StringComparer.Ordinal);
            using var cmd = _conn!.CreateCommand();
            cmd.CommandText = "SELECT path, mtime, entries FROM folders";
            using var reader = cmd.ExecuteReader();
            var pathOrd = reader.GetOrdinal("path");
            var mtimeOrd = reader.GetOrdinal("mtime");
            var entriesOrd = reader.GetOrdinal("entries");
            while (reader.Read())
            {
                result[reader.GetString(pathOrd)] = (reader.GetInt64(mtimeOrd), (int)reader.GetInt64(entriesOrd));
            }

            return result;
        }
    }

    /// <summary>整体替换文件夹快照（每轮扫描一次；遍历不完整时不调用，保留旧快照）</summary>
    public void ReplaceFolderSnapshots(IReadOnlyDictionary<string, (long Mtime, int Entries)> snapshots)
    {
        if (!Enabled)
        {
            return;
        }

        try
        {
            lock (_gate)
            {
                using var tx = _conn!.BeginTransaction();
                Execute("DELETE FROM folders", tx);
                foreach (var (path, snap) in snapshots)
                {
                    Execute("INSERT INTO folders (path, mtime, entries) VALUES ($p, $m, $e)", tx,
                        ("$p", path), ("$m", snap.Mtime), ("$e", snap.Entries));
                }

                tx.Commit();
            }
        }
        catch (Exception ex)
        {
            Poison(ex, "文件夹快照写入失败");
        }
    }

    /// <summary>各 TOML 源文件的 mtime 快照（后台对账比对用）。缓存不可用时返回 null，调用方跳过本轮</summary>
    public IReadOnlyDictionary<string, long>? LoadSourceMtimes()
    {
        if (!Enabled)
        {
            return null;
        }

        lock (_gate)
        {
            EnsureUsable();
            var result = new Dictionary<string, long>(StringComparer.Ordinal);
            using var cmd = _conn!.CreateCommand();
            cmd.CommandText = "SELECT hash, source_mtime FROM items";
            using var reader = cmd.ExecuteReader();
            var hashOrd = reader.GetOrdinal("hash");
            var mtimeOrd = reader.GetOrdinal("source_mtime");
            while (reader.Read())
            {
                result[reader.GetString(hashOrd)] = reader.GetInt64(mtimeOrd);
            }

            return result;
        }
    }

    /// <summary>清空全部镜像并复位注水标记（测试与重建用）</summary>
    public void Clear()
    {
        if (!Enabled)
        {
            return;
        }

        lock (_gate)
        {
            using var tx = _conn!.BeginTransaction();
            Execute("DELETE FROM paths", tx);
            Execute("DELETE FROM tags", tx);
            Execute("DELETE FROM categories", tx);
            Execute("DELETE FROM items", tx);
            tx.Commit();
        }

        WriteMeta(HydratedKey, "0");
        IsHydrated = false;
    }

    public void Dispose()
    {
        lock (_gate)
        {
            // DELETE 模式无 checkpoint 需求；连接关闭即完成落盘（崩溃恢复由 SQLite 回滚日志保证）
            _conn?.Dispose();
            _conn = null;
        }
    }

    // ---------- schema ----------

    private void InitSchema()
    {
        lock (_gate)
        {
            Execute("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)");
            var versionText = ReadMeta("schema_version");
            if (versionText != SchemaVersion.ToString())
            {
                // 版本不符（含首次建库与开发期格式演进）：重建全部表；注水标记保持未置位，
                // 由 MetadataStore 从 TOML 全量重建。缓存是派生数据，无历史格式兼容义务
                Execute("DROP TABLE IF EXISTS items");
                Execute("DROP TABLE IF EXISTS paths");
                Execute("DROP TABLE IF EXISTS tags");
                Execute("DROP TABLE IF EXISTS categories");
                CreateTables();
                WriteMeta("schema_version", SchemaVersion.ToString());
                _logger.LogInformation("元数据缓存 schema 已重建 v{Version}", SchemaVersion);
            }

            IsHydrated = ReadMeta(HydratedKey) == "1";

            // folders 快照表后加（旧缓存无此表）：幂等补建，免于整库重建
            Execute("CREATE TABLE IF NOT EXISTS folders (path TEXT PRIMARY KEY, mtime INTEGER NOT NULL, entries INTEGER NOT NULL)");
        }
    }

    private void CreateTables()
    {
        Execute(
            "CREATE TABLE items (" +
            "hash TEXT PRIMARY KEY, " +
            "url TEXT, " +
            "star INTEGER NOT NULL DEFAULT 0, " +
            "annotation TEXT, " +
            "source_mtime INTEGER NOT NULL DEFAULT 0, " +
            "width INTEGER NOT NULL DEFAULT 0, " +
            "height INTEGER NOT NULL DEFAULT 0, " +
            "palette TEXT, " +
            "palette_version INTEGER NOT NULL DEFAULT 0)");
        Execute("CREATE TABLE paths (hash TEXT NOT NULL, path TEXT NOT NULL, size INTEGER NOT NULL, mtime INTEGER NOT NULL, PRIMARY KEY (hash, path))");
        Execute("CREATE TABLE tags (hash TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY (hash, tag))");
        Execute("CREATE TABLE categories (hash TEXT NOT NULL, category TEXT NOT NULL, PRIMARY KEY (hash, category))");
        Execute("CREATE TABLE folders (path TEXT PRIMARY KEY, mtime INTEGER NOT NULL, entries INTEGER NOT NULL)");
    }

    // ---------- 内部辅助（调用方已持有 _gate） ----------

    private void InsertItem(string hash, ItemMetadata meta, long sourceMtime, SqliteTransaction tx)
    {
        Execute(
            "INSERT INTO items (hash, url, star, annotation, source_mtime, width, height, palette, palette_version) VALUES ($h, $u, $s, $a, $m, $w, $t, $p, $v)",
            tx,
            ("$h", hash), ("$u", meta.Url ?? (object)DBNull.Value), ("$s", meta.Star),
            ("$a", meta.Annotation ?? (object)DBNull.Value), ("$m", sourceMtime),
            ("$w", meta.Width), ("$t", meta.Height), ("$p", (object?)PaletteJson(meta.Palette) ?? DBNull.Value), ("$v", meta.PaletteVersion));
        InsertChildRows(hash, meta, tx);
    }

    /// <summary>调色板的镜像存储格式（与 TOML 同款 entry；null = 未提炼）</summary>
    private static string? PaletteJson(List<PaletteEntry>? palette) =>
        palette is null ? null : System.Text.Json.JsonSerializer.Serialize(palette);

    private static List<PaletteEntry>? ParsePaletteJson(string? json)
    {
        if (json is null)
        {
            return null;
        }

        try
        {
            return System.Text.Json.JsonSerializer.Deserialize<List<PaletteEntry>>(json);
        }
        catch (Exception)
        {
            return null; // 损坏视为未提炼，worker 重新提炼补齐
        }
    }

    private void InsertChildRows(string hash, ItemMetadata meta, SqliteTransaction tx)
    {
        foreach (var p in meta.Paths)
        {
            Execute("INSERT INTO paths (hash, path, size, mtime) VALUES ($h, $p, $s, $m)", tx,
                ("$h", hash), ("$p", p.Path), ("$s", p.Size), ("$m", p.ModificationTime));
        }

        foreach (var tag in meta.Tags)
        {
            Execute("INSERT OR IGNORE INTO tags (hash, tag) VALUES ($h, $v)", tx, ("$h", hash), ("$v", tag));
        }

        foreach (var category in meta.Categories)
        {
            Execute("INSERT OR IGNORE INTO categories (hash, category) VALUES ($h, $v)", tx, ("$h", hash), ("$v", category));
        }
    }

    private void DeleteChildRows(string hash, SqliteTransaction tx)
    {
        Execute("DELETE FROM paths WHERE hash=$h", tx, ("$h", hash));
        Execute("DELETE FROM tags WHERE hash=$h", tx, ("$h", hash));
        Execute("DELETE FROM categories WHERE hash=$h", tx, ("$h", hash));
    }

    private void LoadChildRows(
        string sql,
        string valueColumn,
        Dictionary<string, (ItemMetadata Meta, long SourceMtime)> items,
        Action<ItemMetadata, string> add)
    {
        using var cmd = _conn!.CreateCommand();
        cmd.CommandText = sql;
        using var reader = cmd.ExecuteReader();
        var hashOrd = reader.GetOrdinal("hash");
        var valueOrd = reader.GetOrdinal(valueColumn);
        while (reader.Read())
        {
            if (items.TryGetValue(reader.GetString(hashOrd), out var entry))
            {
                add(entry.Meta, reader.GetString(valueOrd));
            }
        }
    }

    private void EnsureUsable()
    {
        if (_conn is null || _poisoned)
        {
            throw new InvalidOperationException("元数据缓存不可用");
        }
    }

    private void Poison(Exception ex, string what)
    {
        _logger.LogError(ex, "{What}，缓存已熔断（退化为纯 TOML 模式）", what);
        lock (_gate)
        {
            _poisoned = true;
        }
    }

    private string? ReadMeta(string key)
    {
        using var cmd = _conn!.CreateCommand();
        cmd.CommandText = "SELECT value FROM meta WHERE key=$k";
        cmd.Parameters.AddWithValue("$k", key);
        return cmd.ExecuteScalar() as string;
    }

    private void WriteMeta(string key, string value)
    {
        lock (_gate)
        {
            if (_conn is null)
            {
                return;
            }

            using var cmd = _conn.CreateCommand();
            cmd.CommandText = "INSERT INTO meta (key, value) VALUES ($k, $v) ON CONFLICT(key) DO UPDATE SET value=$v";
            cmd.Parameters.AddWithValue("$k", key);
            cmd.Parameters.AddWithValue("$v", value);
            cmd.ExecuteNonQuery();
        }
    }

    private void Execute(string sql, SqliteTransaction? tx = null, params (string Name, object Value)[] parameters)
    {
        using var cmd = _conn!.CreateCommand();
        cmd.CommandText = sql;
        if (tx is not null)
        {
            cmd.Transaction = tx;
        }

        foreach (var (name, value) in parameters)
        {
            cmd.Parameters.AddWithValue(name, value);
        }

        cmd.ExecuteNonQuery();
    }
}
