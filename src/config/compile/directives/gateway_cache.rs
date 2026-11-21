use super::cache_shared::{
    invalid_arguments, invalid_value, mark_duplicate, parse_on_off, parse_positive_usize,
    parse_proxy_cache_use_stale, parse_proxy_cache_valid,
};
use super::*;

#[derive(Debug, Clone, Copy)]
enum GatewayKind {
    FastCgi,
    Uwsgi,
    Scgi,
}

impl GatewayKind {
    const ALL: [Self; 3] = [Self::FastCgi, Self::Uwsgi, Self::Scgi];

    fn prefix(self) -> &'static str {
        match self {
            Self::FastCgi => "fastcgi",
            Self::Uwsgi => "uwsgi",
            Self::Scgi => "scgi",
        }
    }
}

#[derive(Default)]
pub(super) struct GatewayCacheSeen {
    fastcgi: GatewayCacheSeenFields,
    uwsgi: GatewayCacheSeenFields,
    scgi: GatewayCacheSeenFields,
}

#[derive(Default)]
struct GatewayCacheSeenFields {
    cache: bool,
    key: bool,
    valid: bool,
    min_uses: bool,
    max_range_offset: bool,
    methods: bool,
    bypass: bool,
    no_cache: bool,
    lock: bool,
    lock_timeout: bool,
    lock_age: bool,
    revalidate: bool,
    background_update: bool,
    use_stale: bool,
}

pub(super) fn try_apply_gateway_cache_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut GatewayCacheSeen,
) -> ConfigResult<bool> {
    let Some((kind, field)) = classify(&directive.name) else {
        return Ok(false);
    };
    validate_no_block(directive)?;
    let config = match kind {
        GatewayKind::FastCgi => &mut context.fastcgi_cache,
        GatewayKind::Uwsgi => &mut context.uwsgi_cache,
        GatewayKind::Scgi => &mut context.scgi_cache,
    };
    let seen = match kind {
        GatewayKind::FastCgi => &mut seen.fastcgi,
        GatewayKind::Uwsgi => &mut seen.uwsgi,
        GatewayKind::Scgi => &mut seen.scgi,
    };

    match field {
        "cache" => {
            mark_duplicate(directive, &mut seen.cache)?;
            let value = one_arg(directive)?;
            config.cache = if value == "off" {
                ProxyCacheConfig::Off
            } else {
                ProxyCacheConfig::Zone(value.to_string())
            };
        }
        "cache_key" => {
            mark_duplicate(directive, &mut seen.key)?;
            let value = one_arg(directive)?;
            if value.is_empty() {
                return Err(invalid_value(directive, value));
            }
            config.key = value.to_string();
        }
        "cache_valid" => {
            if !seen.valid {
                config.valid.clear();
                seen.valid = true;
            }
            config.valid.push(parse_proxy_cache_valid(directive)?);
        }
        "cache_min_uses" => {
            mark_duplicate(directive, &mut seen.min_uses)?;
            config.min_uses = parse_positive_usize(one_arg(directive)?, directive)?;
        }
        "cache_max_range_offset" => {
            mark_duplicate(directive, &mut seen.max_range_offset)?;
            let value = one_arg(directive)?;
            let parsed = parse_size(value, &directive.location)?;
            if parsed > i64::MAX as u64 {
                return Err(invalid_value(directive, value));
            }
            config.max_range_offset = Some(parsed);
        }
        "cache_methods" => {
            if directive.args.is_empty() {
                return Err(invalid_arguments(directive));
            }
            if !seen.methods {
                config.methods = ProxyCacheMethods::default();
                seen.methods = true;
            }
            for method in &directive.args {
                if !config.methods.insert(method) {
                    return Err(invalid_value(directive, method));
                }
            }
        }
        "cache_bypass" => {
            mark_duplicate(directive, &mut seen.bypass)?;
            if directive.args.is_empty() {
                return Err(invalid_arguments(directive));
            }
            config.bypass = directive.args.clone();
        }
        "no_cache" => {
            mark_duplicate(directive, &mut seen.no_cache)?;
            if directive.args.is_empty() {
                return Err(invalid_arguments(directive));
            }
            config.no_cache = directive.args.clone();
        }
        "cache_lock" => {
            mark_duplicate(directive, &mut seen.lock)?;
            config.lock.enabled = parse_on_off(one_arg(directive)?, directive)?;
        }
        "cache_lock_timeout" => {
            mark_duplicate(directive, &mut seen.lock_timeout)?;
            config.lock.timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "cache_lock_age" => {
            mark_duplicate(directive, &mut seen.lock_age)?;
            config.lock.age = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "cache_revalidate" => {
            mark_duplicate(directive, &mut seen.revalidate)?;
            config.revalidate = parse_on_off(one_arg(directive)?, directive)?;
        }
        "cache_background_update" => {
            mark_duplicate(directive, &mut seen.background_update)?;
            config.background_update = parse_on_off(one_arg(directive)?, directive)?;
        }
        "cache_use_stale" => {
            if !seen.use_stale {
                config.use_stale = ProxyCacheUseStaleConfig::default();
                seen.use_stale = true;
            }
            config.use_stale.flags |= parse_proxy_cache_use_stale(directive)?.flags;
        }
        _ => unreachable!("recognized gateway cache directive"),
    }
    Ok(true)
}

fn classify(name: &str) -> Option<(GatewayKind, &str)> {
    GatewayKind::ALL.into_iter().find_map(|kind| {
        name.strip_prefix(kind.prefix())
            .and_then(|rest| rest.strip_prefix('_'))
            .filter(|field| {
                matches!(
                    *field,
                    "cache"
                        | "cache_key"
                        | "cache_valid"
                        | "cache_min_uses"
                        | "cache_max_range_offset"
                        | "cache_methods"
                        | "cache_bypass"
                        | "no_cache"
                        | "cache_lock"
                        | "cache_lock_timeout"
                        | "cache_lock_age"
                        | "cache_revalidate"
                        | "cache_background_update"
                        | "cache_use_stale"
                )
            })
            .map(|field| (kind, field))
    })
}
