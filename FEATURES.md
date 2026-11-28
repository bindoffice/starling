# Starling 功能、构建与兼容性

[简体中文](FEATURES.md) | [English](FEATURES.en.md)

Starling 是一个用 Rust 实现的 Nginx 兼容 Web 服务器与四层反向代理。它直接读取
Nginx 风格配置，并提供接近 `nginx` 的命令行、PID/信号、静态文件、HTTP 代理、
TLS 与 stream 代理能力。

当前版本是可持续扩展的兼容实现，不声称覆盖 Nginx 全部模块。项目以
`nginx-release-1.31.9/` 中的官方源码和默认配置作为行为参考。

## 源码组织

生产代码按单一职责拆分为子模块，入口文件只保留模块编排和稳定公开接口：

- `config/parser/`：`lexer`、`syntax`、`loader` 分别负责词法、语法树和
  include/glob 装载。
- `config/compile/model/upstream/`：`types`、`server`、`balance` 分别负责
  上游配置、健康状态/并发计数和调度算法。
- `config/compile/model/directives/`：按 access、limits、proxy、rewrite、
  headers、variables 和 expires 拆分配置指令模型，由入口统一重导出。
- `config/compile/http/`：`http` 主上下文编译按状态、指令分派、最终装配和
  递归校验拆分；`http.rs` 只保留编译入口和模块编排。
- `config/compile/access_log.rs` 与 `config/compile/model/access_log.rs`：
  访问日志指令解析、作用域继承模型和多个日志目标的数据结构。
- `config/compile/directives/open_log_file_cache.rs`：日志文件缓存参数校验、
  `off` 语义及作用域实例继承。
- `config/compile/compat/`：集中管理仅保留配置兼容性的指令，以及 rustls
  无法安全等价实现、需要明确拒绝的 TLS 指令。
- `config/compile/syslog.rs`、`config/compile/model/syslog.rs` 与
  `logging/syslog.rs`：syslog 目标参数解析、RFC 3164 报文格式及 UDP/UNIX
  数据报投递。
- `logging/access.rs`：静态/动态日志路径、共享缓冲、定时刷盘和 gzip 压缩写入。
- `logging/file_cache.rs`：`open_log_file_cache` 的按策略实例隔离、LRU 容量控制、
  `inactive`/`min_uses`/`valid` 生命周期、inode 重新校验和日志轮转时缓存失效。
- `logging.rs`：error log 多目标写入、独立级别过滤、文件重开与 reload 重配置。
- `server/stream/error_log.rs`：stream main/server error log 的运行时 logger
  缓存、按默认/选中虚拟 server 选择目标，以及信号重开与 reload 重建。
- `config/compile/stream_pass.rs` 与 `server/stream/pass.rs`：stream `pass`
  的目标解析、本地监听匹配、透传计数和运行时重新分发，避免继续扩大
  `stream/tcp.rs`。
- `config/compile/memcached.rs`、`config/compile/model/memcached.rs` 与
  `server/upstream/memcached/`：Memcached 指令解析、响应模型、协议编解码、
  KEY 转义、超时、命名 upstream keepalive 连接池和 next_upstream 重试分文件实现。
- `server/request/connection.rs`：HTTP/1 连接读取、keep-alive 循环、响应写出与
  lingering close；`request/mod.rs` 只保留子模块编排和导出。
- `server/socket_control.rs`：保存共享 TCP socket 句柄，在 TLS 握手前及最终
  location 确定后应用 `TCP_NODELAY`，避免将底层 socket 操作散落到 HTTP/1、
  TLS 和 HTTP/2 连接代码。
- `config/compile/model/listen.rs` 与 `server/listener_socket.rs`：监听地址/
  socket 参数模型与 TCP/UDP 建绑分离；`listener_socket/platform.rs` 隔离
  Linux `TCP_FASTOPEN`/`TCP_DEFER_ACCEPT` 的平台 socket 操作。
- `server/request/keepalive.rs`：集中计算请求级 keep-alive 决策、配置继承后的
  `Keep-Alive: timeout=` 和连接复用条件。
- `server/request/process/`：常规请求流程与命名 location 子请求流程分离。
- `server/request/process/prepare.rs`：显式执行
  `server rewrite → find location → location rewrite → try_files` 阶段循环，
  `last`/内部重定向不再递归重入整个请求处理器，并由统一的 10 次
  URI change 预算防止 rewrite 环耗尽 worker 栈。
- `server/request/rewrite/`：条件求值、rewrite/return 规则和有序脚本执行
  分离实现；脚本按配置声明顺序执行 `set`、`rewrite`、`return`、`break`
  与 `if`，并以 `None/Continue/Restart/Response` 表示控制流。
- `config/compile/server/rewrite.rs`：在作用域编译完成后，按源码声明顺序
  生成 HTTP server/location 的 rewrite 动作列表，避免分字段执行破坏语义。
- `server/request/post_action.rs`：请求完成后的路径/命名 location 内部执行、
  后台任务隔离、最多 10 次重定向链及原始客户端响应保留。
- `http/request/head.rs`：HTTP/1 请求行与头部有界读取、单行/总量限制、
  非法头部标记及按虚拟主机选择结果应用 `ignore_invalid_headers`。
- `http/response/body.rs`：响应体表示、文件/多段体的内存化边界与长度计算；
  `http/response.rs` 只保留响应控制和公开 API。
- `server/response/sent.rs`：在访问日志阶段同步实际发送语义，包括
  `$sent_http_connection`、`$sent_http_keep_alive`、
  `$sent_http_transfer_encoding` 和 chunked 响应不存在的
  `$sent_http_content_length`。
- `config/compile/directives/msie.rs` 与 `server/response/msie.rs`：
  MSIE/Chrome 错误页 padding、MSIE 刷新页重写及 UA 兼容判断分别负责配置编译
  和响应处理。
- `config/compile/directives/dav.rs`、`config/compile/model/dav.rs` 与
  `server/response/dav/`：WebDAV 指令解析、作用域模型和 PUT/DELETE/MKCOL/
  COPY/MOVE 文件操作分离实现。
- `config/compile/directives/request_headers.rs`：请求头缓冲区与头部策略
  指令解析、HTTP/server 继承和默认值校验。
- `server/early_hints.rs`、`http/response/early_hints.rs` 与
  `server/http2/informational.rs`：`early_hints` 谓词求值、HTTP/1.1 103
  编码和 HTTP/2 informational HEADERS 发送相互独立。
- `config/compile/directives/store.rs`、`config/compile/model/store.rs` 与
  `server/upstream/store.rs`：代理、FastCGI、uWSGI、SCGI 的 `*_store`/
  `*_store_access` 配置解析、作用域继承和响应原子落盘分离实现。
- `config/compile/directives/buffering.rs`、`config/compile/model/upstream_buffering.rs`
  与 `config/compile/temp_path.rs`：四类上游共用的缓冲参数模型、Nginx
  默认值/合并校验，以及请求体和上游临时目录共用的路径层级解析。
- `http/upstream/buffering.rs`：`X-Accel-Buffering` 的独立解析与运行时缓冲
  策略选择，避免把响应头判断继续堆入 proxy/gateway 交换代码。
- `server/upstream/streaming.rs` 与 `server/upstream/proxy_v2/streaming.rs`：
  HTTP/1 与 HTTP/2 upstream 正文/trailer 的增量读取、限速、有界 channel
  背压和下游流式交接；HTTP/2 连接 driver 使用可克隆 guard，保证响应流完成前
  不会被连接对象析构提前中止。
- `server/upstream/tee.rs`、`server/upstream/cache/tee.rs` 与
  `server/upstream/store_stream.rs`：流式正文分叉、缓存条目与 store 文件的
  临时写入/取消清理/完整提交分离实现；下游断开可取消阻塞中的 prepare/write/finish，
  缓存锁随流任务持有到原子提交完成。
- `config/compile/directives/cache.rs`、`config/compile/model/cache.rs` 与
  `server/upstream/cache/`：`proxy_cache_path` 缓存区模型、`proxy_cache*`
  作用域指令、缓存 key、单文件元数据/正文格式、原子落盘、运行时 registry、
  zone 和 keyed cache lock 按职责拆分；`cache/loader.rs` 在缓存区冷启动时
  扫描并校验已有条目、清理损坏文件，`cache/manager.rs` 独立负责
  `inactive`、`max_size` 与 `min_free` 淘汰，`cache/fs.rs` 提供两者共用的
  目录遍历，`cache/manager/disk.rs` 隔离 `statvfs` 空间查询。
- `http/temp_file.rs`、`http/upstream/{buffer,body_reader}.rs` 与
  `server/upstream/gateway/cgi.rs`：请求体/上游响应共用的临时文件 guard、
  上游正文增量缓冲、HTTP chunked/content-length 读取，以及 CGI 头解析和
  正文流式落盘。
- `server/upstream/gateway/streaming.rs` 与
  `server/upstream/gateway/fastcgi/streaming.rs`：FastCGI/uWSGI/SCGI
  流式资格判断、CGI 头增量解析、有界正文 channel，以及 FastCGI
  stdout/stderr/end-request 状态机；与完整缓冲路径共享头解析和响应构造。
- `server/upstream/memcached/protocol.rs` 与
  `server/upstream/memcached/streaming.rs`：Memcached `VALUE` 头/正文/END
  增量解析、有界正文 channel，以及正文与结束标记校验完成后的 keepalive
  连接归还。
- `server/upstream/body_copy.rs`：代理/CGI `*_store` 与代理磁盘缓存共用的
  内存、文件及多段响应体流式复制，避免落盘前再次整包读入内存。
- `server/request/lingering.rs`：HTTP/1.1 连接 lingering close、关闭超时策略和
  超时连接 RST 处理。
- `server/response/ranges/`：Range 解析、单区间切片和
  `multipart/byteranges` 响应的独立实现。
- `server/http2/body.rs`：HTTP/2 响应体流式发送，包括文件、前缀体和多段
  Range 响应。
- `server/http3/`：基于 `quinn` + `h3` 的 QUIC 监听、HTTP/3 请求映射、
  请求体桥接、响应头/正文/trailer 发送，以及 TLSv1.3/ALPN `h3` 配置。
- `server/upstream/proxy_v2/`：HTTP/2 upstream 连接、请求构造、informational
  响应头、正文/trailer、流式正文和 keepalive 连接生命周期分别拆分实现。
- `server/upstream/grpc/`：gRPC 目标解析、请求伪头/正文构造、HTTP/2
  交换、响应头/trailer 过滤和 TLS/连接池分别实现；配置解析位于
  `config/compile/{parsing,directives,model}/grpc*`。
- `tests/config_compat/`：配置兼容测试按访问控制、变量、路由、网关、TLS、
  上游和响应行为拆分。
- `server/tests/`：HTTP/TLS、代理、stream、访问控制和静态文件等较大测试集
  按行为继续拆成子目录，测试入口只负责模块编排。
- `wasm/{mod,abi,runtime}.rs` 与 `config/compile/{directives,model}/wasm.rs`：
  Wasmtime 引擎、编译模块缓存、实例并发限制、core module ABI v1、配置
  校验和 content handler 运行时接入。

新增逻辑应优先放入对应子模块，避免继续堆叠到入口文件；生产代码和测试代码
单文件接近 500 行时，应检查是否可以按职责继续拆分。

## 已实现

- Nginx 配置词法与语法：引号、注释、转义、变量、嵌套块和 `${var}`。
- `include`：相对配置目录解析，支持 `*`、`?`、字符类和循环检测。
- 配置上下文：`main`、`events`、`http`、`server`、`location`、`upstream`、
  `if`、`stream`。
- CLI：`-t`、`-T`、`-q`、`-v`、`-V`、`-s`、`-p`、`-c`、`-e`、`-g`。
- 进程：daemon、PID 文件、`stop`/`quit`/`reload`/`reopen` 信号；reload
  会重载配置、TLS 证书和 error log，监听 socket 变化需要重启。
- 错误日志：可重复 `error_log` 目标、文件/`stderr`、每目标独立级别过滤，
  以及 `syslog:server=...` 的 UDP/UNIX 数据报输出；写入 syslog 时按事件
  级别动态生成 RFC 3164 PRI。
- HTTP/1.0 与 HTTP/1.1：keep-alive、`keepalive_requests`、`keepalive_time`、
  `keepalive_timeout [header_timeout]`（第二参数控制
  `Keep-Alive: timeout=`）、`keepalive_min_timeout`（延迟连接进入
  reusable/waiting 状态，不改变总 keepalive 超时）、
  `keepalive_disable msie6|safari|none`、chunked 请求体、
  `Expect: 100-continue`、Host 校验、`merge_slashes`、
  HTTP/server 级 `client_header_buffer_size`、`large_client_header_buffers`、
  `ignore_invalid_headers`、`underscores_in_headers` 和 `max_headers`
  请求头数量/单行/总量限制；非法头部会按选中虚拟主机的策略忽略或返回
  400，超长请求行返回 414；同时支持请求体限制，以及
  `client_header_timeout`/`client_body_timeout`/`send_timeout`、
  HTTP/server/location 继承的 `client_body_buffer_size`、
  `client_body_temp_path [level1 [level2 [level3]]]`、
  `client_body_in_file_only off|on|clean` 和
  `client_body_in_single_buffer`：超过内存阈值或启用 file-only 时将请求体
  写入临时文件，`clean` 与普通溢出文件在请求结束后删除，`on` 保留文件；
  HTTP/server/location 继承的
  `lingering_close off|on|always`/`lingering_time`/`lingering_timeout`，
  以及超时连接 `reset_timedout_connection` 的 SO_LINGER RST 关闭；支持 TCP 与
  `listen unix:/path` 的 UNIX domain socket 监听。HTTP/server/location 继承的
  `tcp_nodelay on|off`（默认 `on`）在 TLS 握手前按监听默认 server 配置应用，
  并在客户端写响应前按最终 location 配置应用；与 Nginx 一样只负责开启，
  `off` 不会主动关闭已开启的连接选项。HTTP proxy/FastCGI/uWSGI/SCGI/
  Memcached/gRPC 上游建连时也按最终 location 的 `tcp_nodelay` 设置。
  `listen` 的 `backlog=`、`rcvbuf=`、`sndbuf=`、`reuseport`、`bind`、
  `ipv6only=on|off`、`so_keepalive=on|off|idle:interval:count`、
  `fastopen=`、`deferred` 与 Linux `multipath`（MPTCP）会进入统一 socket
  建绑流程；同一地址的重复 socket 参数和重复 `default_server` 会在配置阶段
  拒绝，参数变化会触发 reload 的“监听 socket 变化需要重启”保护。
- HTTP/2：`listen ... ssl http2` 或 http/server 级 `http2 on` 的 TLS ALPN 协商，
  以及明文监听的 HTTP/2 prior-knowledge preface 自动识别；
  `http2_max_concurrent_streams`、`http2_body_preread_size`、
  `http2_chunk_size` 及 Nginx 兼容的内存/索引调优指令，支持并发请求流、
  请求体读取、`max_headers`、连接级响应头过滤，以及静态文件流式响应；并按
  `keepalive_requests`、`keepalive_timeout`、`keepalive_time` 管理 H2 连接
  生命周期和 GOAWAY。`http2_push`、`http2_push_preload`、
  `http2_max_concurrent_pushes`、`http2_max_requests`、`http2_recv_timeout`、
  `http2_idle_timeout`、`http2_max_field_size` 和 `http2_max_header_size` 按
  Nginx 1.31.9 语义作为 obsolete 指令告警并忽略，参数值和重复项不参与解析；
  头部列表上限统一从 `large_client_header_buffers` 派生，并在路由前拒绝超过单个
  buffer 大小的字段；
  request trailers 按 Nginx 语义作为连接级协议错误发送
  `GOAWAY(PROTOCOL_ERROR)`，未消费的普通请求体会后台 drain 后继续复用连接；
  同时保留 HTTP/1.1 回退。
- HTTP/3/QUIC：`listen ... quic` 建立 UDP QUIC 监听，使用 TLSv1.3 和
  ALPN `h3`；支持 HTTP/server 级 `http3 on|off` 配置继承、`$http3` 变量、
  多请求并发、请求体增量读取、现有 router/proxy/static 响应管线、流式响应
  body、请求/响应 trailers 与 HTTP 103 informational response。请求 trailers
  可转发到 HTTP/2 上游；HTTP/1.1 上游使用 chunked 请求体承载 trailers。
  QUIC listener 复用同一地址的 HTTP TLS 证书与 SNI 配置；reload 和 ACME
  证书续期会通过 `Endpoint::set_server_config` 更新现有 endpoint，使新连接
  使用新证书和 TLS 配置，已有连接不受影响。显式 `http3 off` 不会绑定 QUIC
  listener，reload 时移除的 QUIC listener 会关闭对应 endpoint。普通
  HTTP/1.1/HTTP/2 响应会自动添加 `Alt-Svc: h3=":<port>"; ma=86400`，已有
  `Alt-Svc` 响应头时不会覆盖。当前单进程只建立一个 UDP endpoint，所有 QUIC
  数据报由同一个 quinn endpoint driver 收发，0-RTT 关闭。
- PROXY protocol v1/v2：HTTP 与 stream 流式监听的 `listen ... proxy_protocol`，
  支持 v1 的 `TCP4`、`TCP6`、`UNKNOWN`，v2 的 IPv4/IPv6 地址头、TLV 及
  `LOCAL`/不支持命令的无地址语义，以及
  `$proxy_protocol_addr`、`$proxy_protocol_port`、
  `$proxy_protocol_server_addr`、`$proxy_protocol_server_port`；v2 TLV
  支持 `alpn`、`authority`、`unique_id`、`ssl`、`netns`、`ssl_*` 和
  `0xNN` 变量。
- 静态文件：`root`、`alias`、`index`、目录重定向、autoindex 及
  `autoindex_format html|json|jsonp|xml`、单区间与
  `multipart/byteranges` 多区间 Range、`max_ranges`、
  `random_index`、`empty_gif`、`flv` 与 `?start=` 伪流媒体偏移、ETag、
  Last-Modified、`try_files`、带状态保留和覆盖语义的 `error_page`，
  error_page 列表按作用域整体替换继承，以及 HTTP/server/location 继承、
  默认关闭的 `recursive_error_pages`。
- 路径安全：HTTP/server/location 继承的
  `disable_symlinks off|on|if_not_owner [from=...]`，支持变量形式的
  `from=` 前缀跳过；作用于静态文件、index、try_files、gzip_static 和
  flv 候选路径。
- 重定向：默认绝对 `Location`，以及 `absolute_redirect`、
  `server_name_in_redirect`、`port_in_redirect` 的继承和作用域语义。
- WebDAV：`dav_methods put|delete|mkcol|copy|move|off`、
  `create_full_put_path`、`min_delete_depth` 和 `dav_access` 的
  HTTP/server/location 继承；支持 PUT 原子替换、集合创建与递归删除、
  COPY/MOVE 文件和目录、`Destination`、`Depth`、`Overwrite` 校验、
  文件权限与修改时间保留。
- WebAssembly content handler：HTTP 级 `wasm_engine`/`wasm_module_cache`
  与 server/location 级 `wasm_module`、`wasm_phase content`、
  `wasm_timeout`、`wasm_memory_max`、`wasm_fuel`、
  `wasm_request_body_max`、`wasm_response_body_max`、
  `wasm_max_instances`、`wasm_isolation in-process`；使用进程内
  Wasmtime、epoch 墙钟超时、可选 fuel、线性内存/请求体/响应体/响应头/
  实例并发限制、按 canonical path/size/mtime/digest 的编译模块缓存和 reload
  快照切换。实现版本化 core module ABI v1（`starling_abi_version`、
  `starling_alloc`、`starling_handle`），不暴露 WASI 文件、网络、
  环境变量或子进程能力。
- 错误页兼容：HTTP/server/location 继承的 `msie_padding on|off`（默认
  `on`）为 MSIE/Chrome 的 4xx/5xx 内置错误页追加兼容 padding；
  `msie_refresh on|off`（默认 `off`）将 MSIE 的 301/302 响应改为 200
  Refresh HTML。
- 虚拟主机：精确、通配符、正则 `server_name` 和默认 server。
- Location：`=`、`^~`、前缀、正则、命名 location 与嵌套 location。
- Rewrite：`return`、`rewrite`、`set`、`if`、`break`、变量和请求阶段重定向；
  `set`/`rewrite`/`return`/`if` 按配置声明顺序组成单一脚本，`if` 内动作与
  后续规则共享当前变量状态；`last` 重新匹配 location 但不重放 server
  rewrite，无 flag 的 URI rewrite 在脚本结束后重新匹配，`break` 结束脚本
  并保留当前 location；URI 重定向循环按 Nginx 默认上限返回 500；
  `return` 状态码按 Nginx 源码接受 `0..=999`，单参数重定向只接受
  `http://`、`https://` 或 `$scheme` 前缀；`rewrite` 的外部替换同样包含
  `$scheme` 并始终生成 302，即使同时带 `last`/`break`；替换尾部 `?` 会丢弃
  原参数，同时保留其前面的显式查询串；
  `post_action` 支持 HTTP/server/location/`if` 继承、路径或命名
  location 目标、查询串传递和最多 10 次链式执行，执行结果不会替换或回写
  原始客户端响应。
- `mirror`：`mirror uri|off`、`mirror_request_body` 的作用域继承与后台子请求，
  保留原始方法和查询参数，并阻止镜像子请求递归镜像。
- HTTP `CONNECT` 隧道：`tunnel_pass` 支持 server/location/`if` 作用域、server 级继承、
  直接地址和命名 upstream，默认目标为 `$host:$request_port`；支持
  `tunnel_bind`、socket 参数、连接/读写超时、`tunnel_next_upstream` 故障切换、
  原始双向半关闭传播和 `tunnel_buffer_size`。`tunnel_pass https://...` 支持 TLS
  上游，并复用 `proxy_ssl_server_name`、`proxy_ssl_name`、`proxy_ssl_verify`、
  `proxy_ssl_trusted_certificate`、客户端证书及协议/密码套件配置。
  `tunnel_send_lowat` 目前保留配置兼容入口，平台差异仍属于后续兼容项。
- 变量与映射：`map` 的精确、hostname 通配符和正则匹配，`geo` 的
  IPv4/IPv6 CIDR、IPv4 ranges、`default` 与自定义地址变量，以及
  `split_clients` 的 MurmurHash2 确定性分流和 `$arg_*`、`$cookie_*`、
  `$scheme`、`$https`、`$realip_remote_port`、`$proxy_host`、`$proxy_port`、
  `$request_time`、`$request_length`、`$request_body`、
  `$request_body_file`、`$upstream_status`、`$upstream_response_time`、
  动态 `$upstream_http_<name>`/`$sent_http_<name>` 响应头变量与
  `$sent_http_connection`/`$sent_http_keep_alive`/
  `$sent_http_transfer_encoding` 协议生成头变量、
  `$upstream_trailer_<name>`/`$sent_trailer_<name>` trailer 变量、
  `$request_id`、`$hostname` 等常用变量；限流 key 中 `$binary_remote_addr`
  按 Nginx 语义使用 4 字节 IPv4 或 16 字节 IPv6 网络序地址。
- Headers/access：`add_header`、`add_trailer`（HTTP/server/location/if，
  HTTP/1.1 chunked 与 HTTP/2，支持变量、状态过滤和 `always`）、
  HTTP/server/location 继承的 `server_tokens off|on|build`（同步影响
  `Server` 响应头和内置错误页版本信息）、
  `chunked_transfer_encoding on|off`（关闭时不发送 HTTP/1.1 chunked
  trailer；HTTP/2 trailer 不受影响）、
  HTTP/server/location/if 级
  `add_header_inherit`/`add_trailer_inherit` 的 `on|off|merge` 继承模式、
  支持 `off`/`epoch`/`max`、相对时间、
  `@daily`、`modified` 和变量值的 `expires`、
  `if_modified_since off|exact|before`、
  `charset`、`source_charset`、`override_charset`、`charset_types`、
  HTTP 级 `charset_map`（单字节与 UTF-8 双向转换、未映射字符实体回退）、
  `allow`、`deny`。
- 防盗链：`valid_referers` 的 `none`、`blocked`、`server_names`、精确/通配符
  host、host/path 前缀和正则规则，以及 `$invalid_referer` 变量与
  server/location 继承覆盖语义。
- URL 签名：`secure_link`、`secure_link_md5` 和旧版 `secure_link_secret`，
  支持 `$secure_link`、`$secure_link_expires`、过期判断及 HTTP/server/location
  继承覆盖。
- 浏览器变量：`modern_browser`、`ancient_browser` 及对应 value 指令，
  支持浏览器版本比较、`unlisted`、`netscape4`、`$msie`、
  `$modern_browser`、`$ancient_browser` 和作用域继承。
- 用户标识：`userid off|log|v1|on` 及 `userid_name`、`userid_domain`、
  `userid_path`、`userid_expires`、`userid_flags`、`userid_p3p`、
  `userid_mark`，提供 `$uid_got`、`$uid_set`、`$uid_reset` 和 Cookie
  复用/重置语义。
- Access control：`allow`/`deny` 与 `limit_except` 方法级访问规则；该上下文按
  Nginx `NGX_HTTP_LMT_CONF` 只接受 `allow`、`deny`、`auth_basic`、
  `auth_basic_user_file`、`access_log` 和无 URI 部分的 `proxy_pass`；
  `grpc_pass`、`memcached_pass`、`charset`、`post_action` 等指令会明确拒绝。
  未设置 `proxy_pass` 时保留父 location 的代理 handler。
- HTTP Basic Auth：`auth_basic`、`auth_basic_user_file`、`$remote_user`，支持
  `{PLAIN}`、`{SHA}`、`{SSHA}`、APR1、bcrypt 与系统 `crypt()` 密码格式；
  `auth_delay` 可对 401 响应施加固定延迟，降低凭据探测速度。
- 子请求鉴权：`auth_request`、`auth_request_set` 与 `satisfy any|all`。
- HTTP 限流：`limit_req_zone`/`limit_req` 的漏桶速率、burst、delay、nodelay 与
  `limit_conn_zone`/`limit_conn` 并发连接限制，支持自定义状态码、dry run 和
  `$limit_req_status`/`$limit_conn_status`；限流 key 使用原始字节，
  并执行 Nginx 的 65535/255 字节长度上限。
- 运行状态：`stub_status`、`$connections_active`、`$connections_reading`、
  `$connections_writing` 与 `$connections_waiting`。
- Real IP：可信代理 IPv4/IPv6 地址、CIDR 与 `unix:` 对端，
  `X-Real-IP`/`X-Forwarded-For`、`proxy_protocol` 请求头与递归地址解析。
- HTTP 代理：TCP 或 UNIX domain socket `proxy_pass`、路径替换与转义、请求头/请求体控制、
  `proxy_set_header`、`proxy_method`、`proxy_set_body`、`proxy_redirect`、
  `proxy_cookie_domain`、`proxy_cookie_path`、`proxy_cookie_flags`
  （名称/正则匹配、变量标志与 SameSite 规范化）、上游响应头隐藏/透传与
  `proxy_store on|off|path`（仅保存完整 `200` 响应，路径支持变量，相对路径
  基于前缀目录，先写同目录临时文件再原子替换并应用 `Last-Modified`）与
  `proxy_store_access user|group|all` 的 `rw|r` 权限组合、
  `proxy_ignore_headers`、`proxy_pass_trailers` 上游 chunked trailer
  可选透传、`X-Accel-Redirect` 内部重定向、
  `X-Accel-Limit-Rate` 响应限速和 `X-Accel-Charset` 字符集覆盖、
  `proxy_intercept_errors`、`proxy_force_ranges` 字节区间响应（含
  `multipart/byteranges` 多区间）、
  `proxy_limit_rate` 上游响应读取限速、`proxy_connect_timeout`、
  `proxy_send_timeout`、`proxy_read_timeout` 的分阶段超时、
  `proxy_http_version 1.0|1.1|2`（HTTP/2 对明文 upstream 使用 prior knowledge，
  对 HTTPS upstream 使用 ALPN `h2`，支持请求/响应体、上游 trailer 和
  103 Early Hints）、`proxy_buffering off` 或
  `X-Accel-Buffering: no` 下的 HTTP/1.1/HTTP/2 upstream 逐块正文字节流式
  转发（不等待上游 EOF，遵守 `proxy_read_timeout` 与 `proxy_limit_rate`）、
  加权
  round-robin、`least_conn`、`least_time header|last_byte [inflight]`、`ip_hash`、
  `hash [consistent]`、`random [two]`、
  upstream `max_conns`/`max_fails`/`fail_timeout` 被动健康检查、backup 与
  失败切换、`proxy_next_upstream`/`tries`/`timeout` 重试控制，以及
  `early_hints` 谓词过滤上游 HTTP 103 响应并通过 HTTP/1.1 或 HTTP/2 转发，
  且遵循 `proxy_hide_header`/`proxy_pass_header` 的响应头过滤规则，
  upstream 默认启用的 `keepalive` 及
  `keepalive_time`/`keepalive_timeout`/`keepalive_requests` 连接复用，
  `proxy_http_version 2` 的 HTTP/2 upstream 连接复用和同一连接上的并发
  stream，以及
  `proxy_bind` 的 http/server/location 继承、变量求值与 TCP 本地源地址绑定，
  以及 `proxy_socket_keepalive`/`proxy_socket_rcvbuf`/`proxy_socket_sndbuf`
  的 HTTP upstream socket 参数和连接池隔离，还有
  `proxy_pass https://` 的 TLS 建连、
  `proxy_ssl_name`/SNI、`proxy_ssl_session_reuse`、`proxy_ssl_protocols`、`proxy_ssl_ciphers`、
  `proxy_ssl_verify`、`proxy_ssl_verify_depth`、受信证书与 CRL 校验，
  以及 `proxy_ssl_certificate`/`proxy_ssl_certificate_key` 客户端证书和
  `proxy_ssl_password_file` 加密私钥口令；`proxy_headers_hash_max_size`/
  `proxy_headers_hash_bucket_size` 保留作用域继承、默认值和校验，Rust 侧
  不使用 Nginx 的哈希桶实现。`proxy_ssl_session_reuse` 通过共享
  客户端 TLS 配置缓存复用会话，配置 reload 时会清空会话缓存。
- gRPC：`grpc_pass` 支持裸地址、`grpc://`、`grpcs://`、变量和命名 upstream；
  向明文上游使用 HTTP/2 prior knowledge，向 `grpcs://` 上游使用带 ALPN `h2`
  的 TLS。保留原始请求 `:path`，按目标生成 `:scheme`，默认以目标/命名
  upstream 生成 `:authority`，并允许 `grpc_set_header Host` 覆盖；请求体、
  响应体、HTTP/2 trailers 与 `TE: trailers` 均会转发；响应 DATA 与 trailer
  在请求头完成后逐块写回，不等待上游 EOF，HTTP/1.1 下游使用 chunked
  trailer，HTTP/2 下游使用 trailing HEADERS。支持
  `grpc_hide_header`、`grpc_pass_header`、`grpc_ignore_headers`、
  `grpc_intercept_errors`、`grpc_buffer_size`、
  `grpc_connect_timeout`/`send_timeout`/`read_timeout`、
  `grpc_next_upstream`/tries/timeout、`grpc_bind`、socket keepalive/
  rcvbuf/sndbuf，以及命名 upstream `keepalive` 的 HTTP/2 连接复用。
  `grpcs://` 同时支持 SNI/名称、会话复用、协议、密码套件、证书链与 CRL
  校验、客户端证书及加密私钥口令；`grpc_ssl_conf_command` 在 rustls
  后端下会明确报错；`grpc_ssl_certificate_cache` 参数目前仅保留配置兼容
  入口，不会控制内部已存在的 `ClientTlsConfigCache`。
- FastCGI：TCP 或 UNIX domain socket `fastcgi_pass` 与命名 upstream、
  `include fastcgi_params`、
  `fastcgi_param ... if_not_empty`、`fastcgi_index`、
  `fastcgi_split_path_info`/`$fastcgi_path_info`、请求头/请求体转发、
  `fastcgi_intercept_errors`、`fastcgi_force_ranges`、
  `fastcgi_limit_rate`、
  `fastcgi_next_upstream` 重试控制、
  `fastcgi_keep_conn`、`fastcgi_catch_stderr`、连接/读写/发送超时和
  `$fastcgi_script_name`；命名 upstream 配合 `keepalive` 时会复用
  FastCGI 连接，`fastcgi_catch_stderr` 命中 stderr 会返回无效上游响应；
  `fastcgi_hide_header`/`fastcgi_pass_header` 控制响应头过滤，
  `fastcgi_ignore_headers` 支持 Nginx 的忽略项，并默认执行
  `X-Accel-Redirect` 内部重定向、`X-Accel-Limit-Rate` 限速和
  `X-Accel-Charset` 字符集覆盖；
  `fastcgi_bind`、`fastcgi_socket_keepalive`、`fastcgi_socket_rcvbuf`/
  `fastcgi_socket_sndbuf` 支持作用域继承、变量求值和连接池隔离；
  `fastcgi_store`/`fastcgi_store_access` 支持完整 `200` 响应按变量路径原子落盘；
  支持 `Status`、`Location`、`Content-Type` 等 CGI 响应头映射；
  `fastcgi_buffering off` 或 `X-Accel-Buffering: no` 下会在 CGI 头完成后
  逐条转发 `FCGI_STDOUT`，同时继续处理 `FCGI_STDERR` 与
  `FCGI_END_REQUEST`，命中 `fastcgi_catch_stderr` 时终止响应流。
- uWSGI 与 SCGI：TCP 或 UNIX domain socket `uwsgi_pass`/`scgi_pass` 与命名 upstream、
  `suwsgi://`/`uwsgi_ssl_*` 的 uWSGI TLS 建连（含 SNI、证书校验、CRL、
  客户端证书、协议/密码套件和私钥口令），
  `include uwsgi_params`/`scgi_params`、参数与 `if_not_empty`、请求头/请求体
  转发、`uwsgi_string` 字面量尾串、uWSGI modifier、`*_next_upstream`
  重试控制、连接/读写/发送超时和
  错误页拦截、`*_force_ranges`、`*_hide_header`/`*_pass_header` 响应头过滤、
  `*_limit_rate`、`*_ignore_headers`、`X-Accel-Redirect` 内部重定向、
  `X-Accel-Limit-Rate` 限速和 `X-Accel-Charset` 字符集覆盖，以及
  `uwsgi_bind`/`scgi_bind`、
  `uwsgi_socket_keepalive`/`scgi_socket_keepalive`、
  `uwsgi_socket_rcvbuf`/`uwsgi_socket_sndbuf` 和
  `scgi_socket_rcvbuf`/`scgi_socket_sndbuf`；
  `uwsgi_store`/`scgi_store` 及对应 `*_store_access` 支持与 FastCGI 相同的
  响应落盘和权限语义；
  `uwsgi_buffering off`/`scgi_buffering off` 或
  `X-Accel-Buffering: no` 下会在 CGI 头完成后逐块写回正文；
  支持 `$uwsgi_script_name`/`$scgi_script_name`。
- Memcached：`memcached_pass` 支持 TCP、UNIX domain socket 和命名 upstream；
  `$memcached_key` 按 Nginx 规则转义控制字符、空格与 `%`，仅接受 `GET`/`HEAD`；
  `VALUE`/`END` 响应、正文长度和 trailer 均做严格校验，未命中返回 404；
  `memcached_gzip_flag` 命中时添加 `Content-Encoding: gzip`。支持
  `memcached_bind`、`memcached_socket_keepalive`、
  `memcached_connect_timeout`/`send_timeout`/`read_timeout`、
  `memcached_buffer_size`、`memcached_next_upstream`（`error`、`timeout`、
  `invalid_response`、`not_found`、`off`）及其 tries/timeout 指令，
  默认启用 Nginx 的 404/错误页拦截和 force-ranges 语义；命名 upstream
  配置 `keepalive` 后会复用完整 `VALUE` 与 `END` 响应后的后端连接。
  普通 `GET` 在解析 `VALUE` 长度后立即逐块写回正文，后台继续校验正文长度和
  `\r\nEND\r\n` 结束标记；成功后连接可归还 keepalive 池，`HEAD`、Range
  请求或需要完整正文的过滤功能会回退到完整缓冲。
- 上游字节区间：仅 `*_force_ranges on` 会为已缓冲且状态为 200 的代理/CGI 响应
  启用 `Range`；单区间返回 `Content-Range`，多区间返回
  `multipart/byteranges`，并遵循 `max_ranges`。上游自行返回的
  `Accept-Ranges` 头不会单独启用切片。
- 上游读取限速：`proxy_limit_rate`、`fastcgi_limit_rate`、`uwsgi_limit_rate`
  和 `scgi_limit_rate` 在响应体读取阶段按累计字节速率施加异步背压，支持
  http/server/location 继承、变量求值和 `0`（关闭）。
- 上游响应缓冲配置：`proxy_*`、`fastcgi_*`、`uwsgi_*` 与 `scgi_*` 的
  `*_buffering`、`*_buffers`、`*_buffer_size`、`*_busy_buffers_size`、
  `*_temp_path`、`*_max_temp_file_size` 和 `*_temp_file_write_size`
  已按 Nginx 默认值、作用域继承及组合约束解析校验；`*_buffer_size`
  同时限制上游/CGI 响应头，超过上限返回 `502`。响应体先写入
  `*_buffers` 对应的内存额度，超过后按 `*_temp_path` 的哈希目录和
  `*_temp_file_write_size` 分块写入临时文件；超过内存额度与
  `*_max_temp_file_size` 总和时返回 `502`，响应发送完成后删除临时文件。
  `proxy_store` 及三类 CGI store 可直接复制 file-backed 响应体。
  HTTP/1.1 与 HTTP/2 upstream 在 `proxy_buffering off` 或
  `X-Accel-Buffering: no` 且不涉及 Range、trailer、gzip/gunzip、
  charset、sub_filter、addition、SSI 或错误页拦截时，会逐块桥接到下游
  HTTP/1.1 chunked 或 HTTP/2 DATA，使用有界 channel 对上下游施加背压；
  直通 producer 会同时监听下游 receiver 关闭，下游写失败后立即取消仍在等待
  upstream I/O 的正文任务；
  `X-Accel-Buffering: yes` 强制使用配置中的内存/临时文件缓冲，
  `*_ignore_headers X-Accel-Buffering` 可恢复配置默认值。`no` 同时禁用
  代理缓存；缓存与 store 在流式透传时通过 Tee 有界异步队列分叉落盘，临时文件只有
  在上游完整结束且下游未提前断开时才原子 rename。gRPC 响应通过同一 HTTP/2
  upstream 流式管道直接写回；
  FastCGI、uWSGI、SCGI 也在 CGI 响应头完成后接入相同的逐块写回路径，
  FastCGI 流式响应期间继续处理 stderr 和结束记录，且该连接不会归还
  keepalive 池；Memcached `VALUE` 正文按声明的长度逐块写回，尾随
  `END` 校验完成后才复用连接；需要完整响应正文的功能仍沿用完整缓冲；
  `*_busy_buffers_size` 已作为 `BodyWindow` 的按字节发送窗口，限制内存和
  file-backed 数据同时处于下游 busy 状态的额度；下游断开时会取消等待数据、容量
  或外部 I/O 的 producer 并清理临时文件；逐 buffer 的输出链语义仍需继续与
  Nginx 对齐。
- HTTP 代理磁盘缓存：支持 http 级 `proxy_cache_path`（`levels`、
  `keys_zone`、`inactive`、`max_size`、`min_free`、`use_temp_path` 及
  loader/manager 兼容参数）以及 http/server/location 级 `proxy_cache`、
  `proxy_cache_key`、`proxy_cache_valid`、`proxy_cache_min_uses`、
  `proxy_cache_max_range_offset`、`proxy_cache_methods`、
  `proxy_cache_convert_head`、
  `proxy_cache_bypass`、`proxy_no_cache`、`proxy_cache_lock`、
  `proxy_cache_lock_timeout`、`proxy_cache_lock_age`、
  `proxy_cache_revalidate`、`proxy_cache_background_update`、
  `proxy_cache_use_stale`，提供 `$upstream_cache_status` 的 `MISS`、`HIT`、
  `EXPIRED`、`STALE`、`UPDATING`、`REVALIDATED`、`BYPASS`。
  缓存 key 使用 SHA-1 文件名并按 `levels` 建目录；正文与元数据写入同目录
  临时文件后原子重命名，命中时直接发送 file-backed 正文，不重新整包读入
  内存。缓存时间戳使用毫秒精度，`proxy_cache_valid` 的 `ms` 值不会被放大为
  至少一秒；条目格式版本为 `STRLC004`，旧版本缓存文件会被 loader 清理。
  默认缓存 `GET`/`HEAD`，可通过 `proxy_cache_methods` 增加 `POST`；
  `proxy_cache_convert_head` 默认将可缓存的 `HEAD` 回源转换为 `GET`，
  与 `GET` 共用缓存条目；转换后的正文会先完整缓冲，避免下游 HEAD 不读取正文时
  只写入不完整的缓存条目或丢弃 cache Tee。仅缓存完整响应，并遵循
  `Cache-Control`、`Expires`、
  `Set-Cookie`、`Vary: *` 与 `X-Accel-Expires` 的禁缓存/有效期语义；
  `Cache-Control` 按 Nginx 规则优先采用 `s-maxage`，非法或为零的
  `max-age`/`s-maxage` 不写入缓存，非法 `X-Accel-Expires` 保留配置回退，
  `@0` 按 Nginx 语义清除上游覆盖并回退到 `proxy_cache_valid`。
  `Cache-Control: stale-while-revalidate` 可在
  `proxy_cache_background_update on` 时触发后台刷新，并允许更新期间的并发
  请求使用 stale 响应；`stale-if-error` 在上游连接失败窗口内回退到 stale
  响应，无需额外配置 `proxy_cache_use_stale`。
  `proxy_cache_max_range_offset` 默认不限制；设置后按 Nginx 语义仅阻止起点
  不小于阈值的 `bytes=` 范围响应写入缓存，后缀范围不写入，`0` 阻止所有
  带 `Range` 的响应写缓存，但不影响已有缓存读取。
  后台 cache manager 每秒扫描缓存区，按 `inactive` 删除长期未访问条目，
  过期条目在 inactive 前保留以供 stale 回退；命中时会刷新文件 mtime；
  `max_size` 与 Unix `min_free` 按最久未使用顺序淘汰，单轮删除数量和间隔
  遵循 `manager_files`/`manager_sleep`。`keys_zone` 大小作为缓存索引的
  内存额度，loader 按额度装载已有条目，超出额度的文件会被清理；未变化的
  zone 在 reload 时复用，保留索引和 `min_uses` 计数。`proxy_cache_lock`
  在进程内按缓存 key 合并并发 miss；等待超过 `lock_timeout` 的请求回源但不写缓存，持有
  超过 `lock_age` 时允许一个等待请求接管。`proxy_cache_use_stale` 支持
  `error`、`timeout`、`invalid_header`、`updating`、`http_403`、`http_404`、
  `http_429`、`http_500`、`http_502`、`http_503`、`http_504`。
  `proxy_cache_revalidate on` 会在过期条目回源时发送缓存的
  `Last-Modified`/`ETag` 条件头；上游返回 `304` 时继续发送缓存正文，并按
  缓存状态配置或 `304` 新鲜度头刷新有效期。配合
  `proxy_cache_use_stale updating` 时，`proxy_cache_background_update on`
  会立即返回 `STALE` 正文，并由内部更新锁保护的独立任务刷新缓存；
  更新期间的并发请求返回 `UPDATING`。Nginx OSS 本身不提供
  `proxy_cache_path ... purge=` 参数，因此 Starling 与 OSS 配置检查一致地
  将其报告为非法参数。
- CGI 网关磁盘缓存：FastCGI、uWSGI 与 SCGI 复用 HTTP 代理缓存的
  文件格式、zone/index、loader/manager 与 keyed lock 实现，支持对应
  `*_cache_path`、`*_cache`、`*_cache_key`、`*_cache_valid`、
  `*_cache_min_uses`、`*_cache_max_range_offset`、`*_cache_methods`、
  `*_cache_bypass`、`*_no_cache`、`*_cache_lock*`、
  `*_cache_revalidate`、`*_cache_background_update` 和 `*_cache_use_stale`，
  并提供相同的 `$upstream_cache_status` 状态、304 revalidate、过期回退和
  后台更新语义。
- 压缩：`gzip`、`gzip_types`、`gzip_min_length`、`gzip_comp_level`、
  `gzip_vary`、`gzip_proxied`、`gzip_http_version 1.0|1.1`、支持大小写
  不敏感正则和内置 `msie6` 的 `gzip_disable`，以及
  `gzip_static on|always|off` 预压缩文件；未实现 degradation 检测，
  因此 `gzip_disable degradation` 会在配置检查阶段明确报错。
- 解压：`gunzip on|off` 在客户端不接受 gzip 时解压 gzip 响应，并修正
  `Content-Encoding`、`Content-Length`、`Accept-Ranges`、ETag 和 `Vary`。
- 响应替换：`sub_filter`、`sub_filter_types`、`sub_filter_once` 与
  `sub_filter_last_modified`。
- 响应追加：`add_before_body`、`add_after_body` 与 `addition_types`，支持
  server/location 继承、变量 URI、子请求拼接，并修正内容长度、范围和 ETag。
- SSI：`ssi on|off`、`ssi_silent_errors`、`ssi_ignore_recycled_buffers`、
  `ssi_min_file_chunk`、`ssi_value_length`、`ssi_types`、
  `ssi_last_modified`，支持 `include virtual|file`、`echo`、`config`、
  `set`、`if`/`elif`/`else`、正则捕获、`block`/`stub` 和变量求值。
- 访问日志：HTTP 与 stream `access_log`、`log_format`（含
  `escape=default|json|none` 的 Nginx 兼容变量转义）、同作用域多个
  `access_log` 目标、子作用域整体替换继承目标、`access_log off`、
  日志路径变量、`buffer=`/`flush=` 缓冲刷盘、`gzip[=level]` 压缩、
  同文件冲突参数校验、未知/重复日志格式的配置期校验、`access_log ... if=`
  独立条件过滤、`log_not_found` 抑制语义，以及
  HTTP/server/location 与 stream main/server 继承的 `open_log_file_cache`
  （`off` 或 `max=`、`inactive=`、`min_uses=`、`valid=` 参数）、
  `syslog:server=...` 的 UDP/UNIX 数据报目标、`facility=`/`severity=`/
  `tag=`/`nohostname` 参数和 RFC 3164 报文格式。
- TLS：`listen ... ssl`、证书/私钥加载、rustls、SNI 证书选择、
  `ssl_protocols` 的 HTTP/stream 继承与 TLSv1.2/TLSv1.3 选择、按 SNI
  选择 HTTPS 虚拟主机的 TLS/ALPN 配置、`ssl_ciphers`/`proxy_ssl_ciphers`
  的常用 OpenSSL 表达式、`ssl_prefer_server_ciphers`、双向 TLS 的
  `ssl_verify_client`/`ssl_verify_depth`/`ssl_client_certificate`/
  `ssl_trusted_certificate`/`ssl_crl` 客户端证书吊销检查，
  `$ssl_protocol`/`$ssl_cipher`/
  `$ssl_server_name`/`$ssl_alpn_protocol`/`$ssl_client_verify` 及
  `$ssl_client_cert`、`$ssl_client_escaped_cert`、`$ssl_client_fingerprint`、
  `$ssl_client_serial`、客户端证书 DN 和有效期变量，
  以及 HTTP/1.1/HTTP/2 ALPN。HTTP/server 与 stream main/server 支持
  `ssl_stapling on|off`、`ssl_stapling_verify on|off`、
  `ssl_stapling_file`、`ssl_stapling_responder`、`resolver` 和
  `resolver_timeout`：从证书 AIA（或显式 responder）获取 HTTP 或 HTTPS
  OCSP 响应，HTTPS responder 使用 rustls 按 `ssl_trusted_certificate`
  或系统 CA bundle 校验证书链；随后按 trusted certificate 校验 OCSP
  响应签名并动态热替换 rustls `CertifiedKey.ocsp`。刷新时间为
  `nextUpdate - 5m`，限制在 5 分钟到 1 小时之间，失败保留旧响应并 5 分钟后
  重试。`ssl_ocsp on|leaf` 配合 `ssl_ocsp_responder`、`ssl_ocsp_cache`
  对 HTTP/1.1、HTTP/2、HTTP/3 与 stream 下游客户端证书执行握手后的在线
  吊销检查；HTTP/3 同时暴露与 HTTP/1.1、HTTP/2 一致的客户端证书变量。
- 自动 HTTPS（默认开启）：`auto_https on|off` 支持
  `cert_dir=`、`email=`、`staging=on|off`、`acme_directory=`、
  `acme_ca_file=`、`challenge=http-01|dns-01`、`dns_hook=`、
  `dns_propagation_delay=`、`redirect=on|off`、`http_port=`、`https_port=` 参数，
  也可使用对应的 `auto_https_*` 独立指令。启用后为监听 HTTP 端口的
  server 自动增加 HTTPS 监听、生成启动占位证书、在 80 端口提供
  HTTP-01 校验响应并默认返回 308 HTTPS 跳转；公网名称通过 ACME v2
  自动申请并在后台检查/续期，证书原子落盘后热更新 rustls 配置。
  默认使用 HTTP-01；配置 `challenge=dns-01` 后支持公网通配符名称。
  DNS-01 通过 `dns_hook` 调用外部可执行文件发布和清理 TXT 记录，
  调用时设置 `STARLING_ACME_DNS_ACTION=present|cleanup`、
  `STARLING_ACME_DNS_NAME`、`STARLING_ACME_DNS_VALUE` 和
  `STARLING_ACME_DNS_DOMAIN` 环境变量，并向 stdin 写入同内容 JSON。
  所有待验证记录会先发布，等待 `dns_propagation_delay`（默认 30 秒）
  后再触发校验，全部结束后统一清理；同一记录名的多条 TXT 值会分别
  调用 `present`/`cleanup`，hook 应追加和按值删除而不是覆盖整组记录。
  hook 默认 60 秒超时。
  `localhost`、IP、非 FQDN 以及 `.localhost`/`.local`/`.test`/`.invalid`/
  `.example` 等开发名称使用本地 CA（默认
  `auto_https/local-ca.crt`），可信后浏览器不再告警。HTTP-01 不支持
  公网通配符名称；同时包含本地和公网名称的 server 会跳过 ACME 并记录
  告警，避免用一张证书覆盖两类名称。可通过
  `auto_https_acme_directory` 或 `staging on` 接入 Pebble、Let's Encrypt
  staging 等测试目录。完整配置、DNS hook 契约和排障说明见
  `docs/auto-https.md`。
- Stream：TCP 与 `listen unix:/path` 流式监听、TCP 或 UNIX domain socket 上游
  `proxy_pass`、upstream 负载均衡、`proxy_timeout`、`return`、`pass`、`set`、
  `proxy_next_upstream on|off`、`proxy_next_upstream_tries`、
  `proxy_next_upstream_timeout` 的 main/server 继承及建连阶段失败切换
  （开始双向转发后不会重试），
  `map`/`geo`/`split_clients` 流变量与 stream main/server 级
  `allow`/`deny` 访问控制，`limit_conn_zone`/`limit_conn` 的 TCP/UDP
  会话并发限制、dry run、日志级别和 `$limit_conn_status`，
  `set_real_ip_from` 的 stream main/server 继承及按可信 PROXY protocol
  来源改写 `$remote_addr`/`$remote_port`，同时提供
  `$realip_remote_addr`/`$realip_remote_port`，`ssl_preread on` 的
  ClientHello 预读、`$ssl_preread_protocol`/`$ssl_preread_server_name`/
  `$ssl_preread_alpn_protocols`、`preread_timeout`/`preread_buffer_size`
  继承，按 SNI 选择精确、通配符或正则 `server_name` 的 stream 虚拟 server
  并遵循 `default_server`，以及 `proxy_pass` 目标变量和动态 upstream 名称求值，向上游发送
  `proxy_protocol on|v2`（v1、带 CRC32C 的 v2、TCP/TLS 建连前写入及 UDP
  首包写入）、`proxy_protocol_timeout` 的 main/server 继承与覆盖（默认 30s），
  `proxy_half_close` 的 main/server 继承及单侧 EOF/双向半关闭语义，
  UDP `proxy_requests`/`proxy_responses` 数据报数量限制，
  以及 `proxy_upload_rate`/`proxy_download_rate` 的 TCP/UDP 双向限速和变量求值，
  `proxy_buffer_size` 的 main/server 继承与 TCP/UDP relay 缓冲区控制，
  `proxy_downstream_buffer`/`proxy_upstream_buffer` 兼容别名，以及
  `proxy_socket_keepalive`、`proxy_socket_rcvbuf`、`proxy_socket_sndbuf`
  的 upstream socket 参数配置，`proxy_bind` 的 main/server 继承、变量求值及
  TCP/UDP 本地源地址绑定（`transparent` 会明确报错），
  stream TLS 与上游 TLS 指令的 main/server 继承（包括证书、校验、协议、
  密码套件、SNI 和 ALPN 配置），
  以及 `listen ... ssl` 的客户端 TLS 终止、按 SNI 选择 stream 虚拟 server
  及其证书、TLS 协议、密码套件和客户端证书校验配置、`proxy_ssl on` 配合
  `proxy_ssl_protocols`/`proxy_ssl_ciphers`/`proxy_ssl_alpn` 的上游 TLS，
  `proxy_ssl_name` 的变量求值与 `proxy_ssl_server_name` 的 SNI 开关语义，
  下游 TLS 会话向上游 PROXY protocol v2 发送 SNI、ALPN、TLS 版本和密码套件
  TLV，
  `tcp_nodelay`、`ssl_handshake_timeout`、`ssl_reject_handshake` 与
  `ssl_alpn`，和
  `listen ... udp` 的 datagram 代理、`listen ... proxy_protocol`；
  TCP/UDP `access_log`、`log_format` 及常用会话、upstream、PROXY protocol
  和 SSL 变量；stream main/server 级 `error_log` 继承、覆盖、多目标和
  独立级别过滤；`pass` 支持静态或变量目标、按监听族/端口匹配本地
  TCP/UNIX 监听（含 wildcard 监听）、目标监听自身的 TLS/PROXY protocol
  处理与监听 socket 参数，以及最多 10 次 pass 链的循环保护，并拒绝 UDP
  和带 preread 数据的
  连接。

## 构建

```bash
cargo build --release
cargo test --all-targets
```

生成的可执行文件为 `target/release/starling`。

## 运行

默认前缀和配置路径保持 Nginx 习惯：

```bash
sudo ./target/release/starling
# 等价于：
sudo ./target/release/starling -p /usr/local/nginx -c conf/nginx.conf
```

开发时使用仓库内配置并关闭 daemon：

```bash
./target/release/starling -t -p "$PWD" -c conf/nginx.conf
./target/release/starling -p "$PWD" -c conf/nginx.conf -g 'daemon off;'
```

测试配置并输出所有引入文件：

```bash
./target/release/starling -T -p "$PWD" -c conf/nginx.conf
```

向已启动进程发送信号：

```bash
./target/release/starling -p "$PWD" -c conf/nginx.conf -s reload
```

## CI 与发布

`scripts/ci.sh` 会依次执行格式检查、Clippy、完整测试和 release 构建。
GitHub Actions 的 `ci.yml` 在 push 到 `main`、Pull Request 和手动触发时执行
相同检查，并额外验证发布包生成。

`scripts/package-release.sh` 可生成如下结构的发布包：

```text
starling-<version>-<target>/
├── bin/starling
├── conf/
├── docs/
├── html/
├── logs/
├── README.md
├── README.en.md
├── FEATURES.md
└── FEATURES.en.md
```

`release.yml` 在推送 `v*` 标签时构建 Linux x86_64/aarch64 包，生成
`SHA256SUMS` 并发布到 GitHub Release。标签版本必须与 `Cargo.toml` 一致；
也可以在 Actions 页面手动指定标签触发。

## 配置示例

静态站点、反向代理、虚拟主机和 TLS：

```nginx
worker_processes auto;
error_log logs/error.log notice;
pid logs/nginx.pid;

events {
    worker_connections 4096;
}

http {
    include mime.types;
    default_type application/octet-stream;
    sendfile on;
    keepalive_timeout 65;

    upstream application {
        server 127.0.0.1:9000 weight=3;
        server 127.0.0.1:9001;
    }

    server {
        listen 80 default_server;
        server_name example.com www.example.com;
        root html;
        index index.html;

        location /api/ {
            proxy_pass http://application/;
            proxy_set_header Host $host;
            proxy_set_header X-Real-IP $remote_addr;
        }

        location /assets/ {
            expires 7d;
            add_header Cache-Control "public";
        }

        location = /health {
            return 200 "ok\n";
        }
    }
}
```

TCP 四层代理：

```nginx
stream {
    upstream postgres {
        server 10.0.0.12:5432;
        server 10.0.0.13:5432;
    }

    server {
        listen 5432;
        proxy_pass postgres;
        proxy_timeout 10m;
    }
}
```

## 兼容性边界

以下能力尚未完成，配置中会明确报错而不是静默运行错误行为：

- HTTP/3/QUIC 初始实现边界：响应 trailers、自动 Alt-Svc 注入、QUIC
  endpoint 的 server config reload/ACME 热更新和移除 QUIC listener 已支持；
  请求/响应 trailers、informational response、HTTP/3 客户端证书变量与
  `ssl_ocsp` 集成也已支持；但尚无 0-RTT。新增 QUIC listener 或重绑定 UDP
  地址仍需重启。UDP socket 当前只持有一个 endpoint；增加 Tokio worker
  线程不会增加独立的 QUIC 收包路径。未显式配置 `http3` 时，
  `listen ... quic` 继续隐式启用 HTTP/3；显式 `http3 off` 则不会创建 QUIC
  listener。
- 旧版 SSL/TLS 协议；rustls 仅支持 TLSv1.2 与 TLSv1.3。为兼容存量配置，
  `ssl_protocols` 中的 SSLv2、SSLv3、TLSv1、TLSv1.1 会与 TLSv1.2/TLSv1.3
  一起被接受，但 `-t` 和运行时错误日志会输出 `[warn]`，实际只启用
  TLSv1.2/TLSv1.3。如果列表中没有 TLSv1.2 或 TLSv1.3，配置仍会明确报错。
- `ssl_ciphers`/`proxy_ssl_ciphers`/`grpc_ssl_ciphers`/`uwsgi_ssl_ciphers`
  支持 rustls 提供的
  ECDHE AEAD 套件及常用 OpenSSL 别名、排除和组合语法；常见 DHE、CBC/SHA
  等依赖 OpenSSL 遗留算法的套件会被识别并从实际运行集合中省略，同时在 `-t`
  和运行时错误日志输出 `[warn]`。如果最终没有任何 rustls 支持的套件，配置仍会
  明确报错。
- `proxy_ssl_conf_command` 依赖 OpenSSL `SSL_CONF`，rustls 没有可安全等价的
  任意命令执行接口，因此配置期会明确报错；请改用 `proxy_ssl_protocols`、
  `proxy_ssl_ciphers` 等结构化指令。
- `grpc_ssl_conf_command` 与 `proxy_ssl_conf_command` 相同，会明确报错；
  请改用 `grpc_ssl_protocols`、`grpc_ssl_ciphers`。
- `uwsgi_ssl_conf_command` 同样依赖 OpenSSL `SSL_CONF`，配置期会明确报错；
  请改用 `uwsgi_ssl_protocols`、`uwsgi_ssl_ciphers`。`uwsgi_ssl_certificate_cache`
  目前仅保留配置兼容入口；上游 `ClientConfig` 的进程内复用由内部
  `ClientTlsConfigCache` 负责，但不会读取该指令的 `max=`、`inactive=`、
  `valid=` 或 `off` 参数。
- `ssl_verify_client` 支持 `on`、`optional` 和 `optional_no_ca`，并支持通过
  `ssl_crl` 检查客户端证书吊销状态；HTTP/stream 上游也支持
  `proxy_ssl_crl`、`proxy_ssl_verify_depth`、`uwsgi_ssl_crl`/
  `uwsgi_ssl_verify_depth` 和客户端证书。服务器及上游加密 PEM 私钥可分别通过
  `ssl_password_file`、`proxy_ssl_password_file`、`uwsgi_ssl_password_file`
  解锁。`ssl_stapling_verify on` 需要配置 `ssl_trusted_certificate`，否则
  配置期报错。HTTP/3 客户端证书信息与 `ssl_ocsp` 在线检查已接入，运行时
  边界见上文。
- 下游 HTTP/2 支持 TLS ALPN 和明文 prior knowledge（`listen ... http2` 或
  `http2 on`）。与 Nginx 1.31.9 一致，不支持 HTTP/1.1 `Upgrade: h2c`；HTTP/2
  server push 自 Nginx 1.25.1 起已移除，相关旧指令只做兼容告警并忽略。启用 HTTP/2
  的明文监听在未收到完整 HTTP/2 preface 时仍会回退到 HTTP/1.1。
  HTTP proxy 上游的 `proxy_http_version 2` 与 gRPC upstream 支持明文
  prior knowledge 和 TLS ALPN，并会按命名 upstream 的 `keepalive` 参数复用
  HTTP/2 连接。`proxy_buffering off` 或 `X-Accel-Buffering: no` 的代理响应
  可在请求头完成后立即返回，连接可继续承载并发 stream。普通 HTTP 代理在
  `proxy_request_buffering off` 下会对 HTTP/1.1 与 HTTP/2 upstream 边读边转发
  Content-Length 或 chunked 请求体；gRPC 在 `grpc_request_buffering off`、三类 CGI
  网关在对应 `*_request_buffering off` 下复用同一请求体流式路径，入站 HTTP/2
  请求体也已由 `H2BodyTask` 增量读取；对应 buffering 保持 `on` 时仍完整缓冲。
  gRPC、FastCGI、uWSGI 与 SCGI 响应正文均已支持增量转发。
- 动态模块 `load_module`。
- mail 模块。
- FastCGI 的高级缓存细节；基础转发、UNIX domain socket、磁盘缓存、
  `fastcgi_keep_conn`、命名 upstream keepalive 复用及缓冲配置/响应头
  上限和响应体临时文件已支持，其余高级指令目前仅保留配置兼容入口。
- uWSGI/SCGI 的高级缓存细节和 keepalive；基础网关转发、UNIX domain
  socket、uWSGI `suwsgi://` TLS、缓冲配置/响应头上限和响应体临时文件已支持，
  FastCGI/uWSGI/SCGI 的 `*_cache_path`、`*_cache`、key/valid/methods/lock/
  use_stale 等缓存配置已进入运行时，并支持查询、写入、304 刷新和 stale 回退。
  三个 CGI 网关与 HTTP proxy 已支持响应头 hide/pass、`*_ignore_headers`
  配置、`X-Accel-Redirect` 内部重定向、`X-Accel-Limit-Rate` 响应限速和
  `X-Accel-Charset` 字符集覆盖。HTTP 代理缓存已按 Nginx 语义处理
  `X-Accel-Expires`、`Expires`、`Cache-Control`、`Set-Cookie` 和 `Vary`；
  `X-Accel-Buffering` 已改变运行时内存/临时文件缓冲策略；三类 CGI 网关在
  `*_buffering off` 或 `X-Accel-Buffering: no` 下已接入逐块流式写回。
  网关缓存与 store 可在逐块写回时通过 Tee 落盘；涉及 Range、错误页拦截或
  正文改写过滤时仍回退完整缓冲，以保证响应重写语义。
- Memcached 普通 `GET` 正文已按 `VALUE` 长度逐块写回，并在 `END` 校验成功
  后归还 keepalive 连接；`HEAD`、Range 和正文改写过滤仍回退完整缓冲。
- Nginx OSS 1.31.9 的 stream 模块本身不提供 `if` 和 `limit_req`；Starling
  同样不把这两项作为 stream 兼容能力。基础 stream 访问日志、
  `buffer=`/`flush=`/`gzip`/`if=` 及 syslog 目标已支持。
- stream UDP 按客户端地址复用上游会话，支持多报文和多响应；UDP 上游 TLS
  尚未实现。
- Linux 监听 socket 支持 `reuseport`、`multipath`（MPTCP）、`fastopen=`、
  `deferred`、`ipv6only=`、`rcvbuf=`/`sndbuf=` 与可调 `so_keepalive`；
  FreeBSD 专用的 `setfib=` 会明确拒绝。`accept_filter=` 按 Nginx 在无
  `SO_ACCEPTFILTER` 平台上的方式保留配置兼容入口，但不会改变 socket。
  stream UDP 会拒绝 `backlog`、`fastopen`、`deferred`、`multipath`、
  `so_keepalive` 等仅适用于面向连接 socket 的参数。
- stream upstream 支持 `least_time connect|first_byte|last_byte [inflight]`。
- Nginx rewrite 模块剩余边界（复杂 `if` 分支间完整模块 loc_conf 回退语义）
  和 HTTP cache purge 扩展；cache loader 已支持冷启动扫描、校验、损坏条目
  清理及 `keys_zone` 容量约束。
- 原生的 master/多 worker 进程模型；当前使用一个 master 进程和 Tokio 多线程运行时。

可被配置解析但暂不改变行为的 socket 调优、缓存管理器参数和 SSL 参数会保留
兼容入口。当前包括 `proxy_send_lowat`、`fastcgi_send_lowat`、`gzip_window`、
`gzip_hash`、`gzip_no_buffer`、`postpone_gzipping`、`ssl_certificate_cache`、
`proxy_ssl_certificate_cache`、`grpc_ssl_certificate_cache` 和
`uwsgi_ssl_certificate_cache`；这些参数用于让标准 Nginx 配置通过迁移检查，
参数形式会按 Nginx 约束校验，但不会改变 Starling 的 socket 或 zlib 缓冲策略。
服务端证书只在启动或 reload 构建 TLS 配置时加载，因此 `ssl_certificate_cache`
当前没有运行时效果；三个上游证书缓存指令也不会控制内部
`ClientTlsConfigCache`，该缓存固定最多复用 64 组 `ClientConfig`，并在 reload
时清空。`proxy_send_lowat` 和 `fastcgi_send_lowat` 还会在配置诊断中明确提示
“不生效”，避免静默误判。
生产部署前应使用目标配置执行 `-t`，并通过集成测试验证代理、TLS 和超时行为。

## 代码结构

- `src/config/`：配置 AST、词法/语法分析、include 与编译为运行时模型。
  - `compile/access_log.rs`：访问日志解析与作用域覆盖逻辑。
  - `compile/server/`：HTTP server/location 编排、有序 rewrite 脚本、
    `if`、`limit_except`、监听与路由匹配。
  - `compile/directives/`：按 TLS、响应头、代理/网关缓冲、访问限制、
    rewrite、CGI 和内容模块分派指令。
  - `compile/variables/`：分别编译 `map`、`geo`、`split_clients` 与限流配置。
- `src/http/`：HTTP 请求/响应解析、校验和 upstream 响应解析。
- `src/server/request/`：HTTP 请求生命周期、显式 rewrite 阶段循环、路由、
  访问控制与有序 rewrite 脚本。
- `src/server/response/`：静态文件、错误页、响应头和过滤链。
- `src/server/variables/`：变量求值、map/geo 查询、PROXY protocol TLV 与正则捕获。
- `src/server/limits.rs`：HTTP/stream 共享的连接计数状态与 RAII guard。
- `src/server/upstream/`：代理、gRPC、重试、负载均衡、连接 socket 参数与
  FastCGI/uWSGI/SCGI 网关。
- `src/server/stream/`：stream 路由、访问控制、return、TCP/UDP 转发与日志。
- `src/server/tls/`：HTTP/stream TLS 配置、证书加载和客户端证书校验。
- `src/server/tls/ocsp/`：证书 issuer/AIA 解析、OCSP HTTP 获取、响应校验、
  共享 staple 状态和 `nextUpdate` 刷新循环；`src/server/resolver/` 提供带
  TTL 缓存的 A/AAAA DNS 客户端。
- `src/runtime.rs`：Nginx 风格 CLI、daemon、PID 和信号命令。
- `src/logging.rs`：error log、access log 和时间格式。
- `src/logging/access.rs`：access log 文件缓冲、定时刷新和 gzip 输出。
- `src/logging/file_cache.rs`：access log 文件描述符缓存、LRU 淘汰和轮转校验。

模块按配置上下文、请求阶段或协议职责拆分。入口文件只负责编排和少量粘合；
新增指令族、过滤器或变量类型应进入独立子模块，避免继续扩大单文件。

