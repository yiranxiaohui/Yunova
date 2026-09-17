# SQLite → PostgreSQL：让第二种后端真正可用

## 起因

线上 `10.1.51.1` 用 SQLite 跑，要换 PostgreSQL，并且安装向导要能二选一。

向导里"选 PostgreSQL"这个选项一直存在，但**从来没有真正跑通过**。安装能写完
配置，之后每一个请求都 500。用真实的 Postgres 16 复现，注册接口报：

```
column "expires_at" is of type timestamp with time zone but expression is of type text
```

MySQL 更早，连启动都过不去：

```
FATAL: 1267 Illegal mix of collations ... refusing to start
```

## 真正的原因

连接池是 `sqlx::Any`（运行时选后端），而 `Any` 的类型映射表**只覆盖**
`Bool / SmallInt / Integer / BigInt / Real / Double / Text / Blob`。
拿裸驱动直接探测（`sqlx-core-0.8.6/src/any/row.rs`、`postgres/src/any.rs`）：

| 探测                     | Postgres                                     | MySQL                       |
| ------------------------ | -------------------------------------------- | --------------------------- |
| 时间列 → `String`        | ✗ `does not support the Postgres type Timestamptz` | ✗ `does not support MySql type Datetime` |
| 布尔列 → `i64`           | ✗ `i64 is not compatible with BOOLEAN`       | ✗ `does not support Tiny`   |
| `SUM(bigint)` → `i64`    | ✗ `does not support the Postgres type Numeric` | —                         |
| `MEDIUMTEXT` → `String`  | —                                            | ✗ 被当成 `BLOB`             |

也就是说：只要一张表里有 `TIMESTAMPTZ`，这张表就**没有任何一行能被读出来**，
与 SQL 写得对不对无关。SQLite 侧一直正常，只因为它把时间和布尔都存成
`TEXT` / `INTEGER`——恰好落在 `Any` 支持的集合里。

迁移 `0045` 的注释其实已经写对了结论（"Stored as RFC3339 text rather than a
native timestamp because the pool is `sqlx::Any`"），只是这个约束当时没有回头
套用到前 44 个迁移上。

## 做法

**让 Postgres 的存储域和 SQLite 对齐**，而不是给 `Any` 打补丁：

1. `TIMESTAMPTZ` → `TEXT`，默认值
   `to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')`——和 SQLite 的
   `datetime('now')` **逐字节相同**的格式，因此现有的字典序比较、`substr`
   取日期、Rust 侧的 `parse_from_str` 全部不需要改。
2. `BOOLEAN` → `INT`（0/1），和 SQLite 的 `INTEGER` 一致；
   `bool_as_int()` 之类的方言分支随之退化成恒等函数。
3. `SUM(...)` 一律包 `CAST(... AS BIGINT)`，避开 Postgres 的 `numeric`。
4. 新增迁移 `0048`，把**已经**建成 timestamptz/boolean 的库就地转过来，
   用 `information_schema` 驱动、可重复执行；新库跑到这一步是空循环。

### 为什么允许改已发布的迁移

平时不改历史迁移。这次的例外是：这些文件描述的数据库**不可能存在业务数据**
——建完就用不了。真有人建过，`0048` 会把它转成新形状。两条路都收敛到同一个
schema。

### MySQL 直接删掉

`MEDIUMTEXT` 被 `Any` 当成 BLOB，而 `system_prompt`、`content`、`graph_json`
等等全是 `MEDIUMTEXT`；要修就得把整套表结构和 `Any` 的类型表重新对齐一遍，
而它连启动都过不去，说明没有任何人在用。留着一个"能选、但一定坏"的选项，
比不给这个选项更糟。于是去掉 `DbKind::Mysql`、44 个迁移文件、26 处方言分支、
向导里的卡片和 compose 里的 profile。向导现在就是 SQLite / PostgreSQL 二选一，
和需求一致。

## 数据搬迁

新增 `yunova db-copy --from <url> --to <url>`：在目标库跑完迁移，然后按外键
拓扑序逐表整行复制，最后把 Postgres 的 sequence 推到 `MAX(id)`。

放进产品而不是写成一次性脚本，因为：换后端是每个自托管用户都会遇到的事；
而且复制逻辑必须和 schema 同源，外挂脚本迟早会和迁移脱节。

`--to` 非空时默认拒绝执行（`--allow-nonempty` 才继续），避免把数据灌进一个
已经在用的库。
