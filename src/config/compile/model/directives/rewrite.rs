use super::*;

#[derive(Debug, Clone)]
pub struct ReturnConfig {
    pub status: u16,
    pub value: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RewriteRule {
    pub regex: Regex,
    pub replacement: String,
    pub flag: RewriteFlag,
}

#[derive(Debug, Clone)]
pub enum RewriteAction {
    Set(SetVariable),
    Rewrite(RewriteRule),
    Return(ReturnConfig),
    Break,
    If(Box<ConditionalConfig>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewriteFlag {
    None,
    Last,
    Break,
    Redirect,
    Permanent,
}

#[derive(Debug, Clone)]
pub struct TryFiles {
    pub candidates: Vec<String>,
    pub fallback: String,
}

#[derive(Debug, Clone)]
pub struct ConditionalConfig {
    pub condition: Condition,
    pub actions: Vec<RewriteAction>,
    pub explicit_content_handler: Option<ContentHandlerKind>,
    pub ssi: SsiConfig,
    pub return_directive: Option<ReturnConfig>,
    pub rewrites: Vec<RewriteRule>,
    pub proxy_pass: Option<ProxyPassConfig>,
    pub tunnel_pass: Option<TunnelPassConfig>,
    pub grpc_pass: Option<GrpcPassConfig>,
    pub fastcgi_pass: Option<FastCgiPassConfig>,
    pub uwsgi_pass: Option<UwsgiPassConfig>,
    pub scgi_pass: Option<ScgiPassConfig>,
    pub memcached_pass: Option<MemcachedPassConfig>,
    pub root: PathBuf,
    pub alias: Option<PathBuf>,
    pub sendfile: bool,
    pub limit_rate: Option<String>,
    pub limit_rate_after: Option<String>,
    pub error_pages: Vec<ErrorPage>,
    pub post_action: Option<String>,
    pub gzip: bool,
    pub charset: CharsetConfig,
    pub access_log: AccessLogConfig,
    pub add_headers: Vec<AddHeader>,
    pub add_header_inherit: HeaderInheritance,
    pub add_trailers: Vec<AddHeader>,
    pub add_trailer_inherit: HeaderInheritance,
    pub expires: Option<ExpiresRule>,
    pub access_rules: Vec<AccessRule>,
    pub sets: Vec<SetVariable>,
    pub rewrite_log: bool,
    pub uninitialized_variable_warn: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentHandlerKind {
    FastCgi,
    Uwsgi,
    Scgi,
    Grpc,
    Memcached,
    Proxy,
    Tunnel,
    Wasm,
}

#[derive(Debug, Clone)]
pub enum Condition {
    Truthy(String),
    Equal {
        left: String,
        right: String,
        negate: bool,
    },
    Regex {
        value: String,
        regex: Regex,
        negate: bool,
    },
    FileTest {
        operation: FileTest,
        path: String,
        negate: bool,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum FileTest {
    Exists,
    File,
    Directory,
    Executable,
}
