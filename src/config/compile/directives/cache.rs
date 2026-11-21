use super::cache_shared::{
    invalid_arguments, invalid_value, mark_duplicate, parse_on_off, parse_proxy_cache_use_stale,
    parse_proxy_cache_valid,
};
use super::*;

#[derive(Default)]
pub(super) struct ProxyCacheSeen {
    pub(super) cache: bool,
    pub(super) key: bool,
    pub(super) valid: bool,
    pub(super) min_uses: bool,
    pub(super) max_range_offset: bool,
    pub(super) methods: bool,
    pub(super) convert_head: bool,
    pub(super) bypass: bool,
    pub(super) no_cache: bool,
    pub(super) lock: bool,
    pub(super) lock_timeout: bool,
    pub(super) lock_age: bool,
    pub(super) revalidate: bool,
    pub(super) background_update: bool,
    pub(super) use_stale: bool,
}

pub(super) fn try_apply_proxy_cache_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut ProxyCacheSeen,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "proxy_cache" => {
            mark_duplicate(directive, &mut seen.cache)?;
            validate_no_block(directive)?;
            let value = one_arg(directive)?;
            context.proxy_cache = if value == "off" {
                ProxyCacheConfig::Off
            } else {
                ProxyCacheConfig::Zone(value.to_string())
            };
        }
        "proxy_cache_key" => {
            mark_duplicate(directive, &mut seen.key)?;
            validate_no_block(directive)?;
            let value = one_arg(directive)?;
            if value.is_empty() {
                return Err(invalid_value(directive, value));
            }
            context.proxy_cache_key = value.to_string();
        }
        "proxy_cache_valid" => {
            validate_no_block(directive)?;
            if !seen.valid {
                context.proxy_cache_valid.clear();
                seen.valid = true;
            }
            context
                .proxy_cache_valid
                .push(parse_proxy_cache_valid(directive)?);
        }
        "proxy_cache_min_uses" => {
            mark_duplicate(directive, &mut seen.min_uses)?;
            validate_no_block(directive)?;
            let value = one_arg(directive)?;
            context.proxy_cache_min_uses = value
                .parse::<usize>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| invalid_value(directive, value))?;
        }
        "proxy_cache_max_range_offset" => {
            mark_duplicate(directive, &mut seen.max_range_offset)?;
            validate_no_block(directive)?;
            let value = parse_size(one_arg(directive)?, &directive.location)?;
            if value > i64::MAX as u64 {
                return Err(invalid_value(directive, one_arg(directive)?));
            }
            context.proxy_cache_max_range_offset = Some(value);
        }
        "proxy_cache_methods" => {
            validate_no_block(directive)?;
            if directive.args.is_empty() {
                return Err(invalid_arguments(directive));
            }
            if !seen.methods {
                context.proxy_cache_methods = ProxyCacheMethods::default();
                seen.methods = true;
            }
            for method in &directive.args {
                if !context.proxy_cache_methods.insert(method) {
                    return Err(invalid_value(directive, method));
                }
            }
        }
        "proxy_cache_convert_head" => {
            mark_duplicate(directive, &mut seen.convert_head)?;
            validate_no_block(directive)?;
            context.proxy_cache_convert_head = parse_on_off(one_arg(directive)?, directive)?;
        }
        "proxy_cache_bypass" => {
            mark_duplicate(directive, &mut seen.bypass)?;
            validate_no_block(directive)?;
            if directive.args.is_empty() {
                return Err(invalid_arguments(directive));
            }
            context.proxy_cache_bypass = directive.args.clone();
        }
        "proxy_no_cache" => {
            mark_duplicate(directive, &mut seen.no_cache)?;
            validate_no_block(directive)?;
            if directive.args.is_empty() {
                return Err(invalid_arguments(directive));
            }
            context.proxy_no_cache = directive.args.clone();
        }
        "proxy_cache_lock" => {
            mark_duplicate(directive, &mut seen.lock)?;
            validate_no_block(directive)?;
            context.proxy_cache_lock.enabled = parse_on_off(one_arg(directive)?, directive)?;
        }
        "proxy_cache_lock_timeout" => {
            mark_duplicate(directive, &mut seen.lock_timeout)?;
            validate_no_block(directive)?;
            context.proxy_cache_lock.timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "proxy_cache_lock_age" => {
            mark_duplicate(directive, &mut seen.lock_age)?;
            validate_no_block(directive)?;
            context.proxy_cache_lock.age =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "proxy_cache_revalidate" => {
            mark_duplicate(directive, &mut seen.revalidate)?;
            validate_no_block(directive)?;
            context.proxy_cache_revalidate = parse_on_off(one_arg(directive)?, directive)?;
        }
        "proxy_cache_background_update" => {
            mark_duplicate(directive, &mut seen.background_update)?;
            validate_no_block(directive)?;
            context.proxy_cache_background_update = parse_on_off(one_arg(directive)?, directive)?;
        }
        "proxy_cache_use_stale" => {
            if !seen.use_stale {
                context.proxy_cache_use_stale = ProxyCacheUseStaleConfig::default();
                seen.use_stale = true;
            }
            context.proxy_cache_use_stale.flags |= parse_proxy_cache_use_stale(directive)?.flags;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

pub(in crate::config::compile) fn validate_cache_zones(
    context: &EffectiveConfig,
    zones: &HashMap<String, Arc<ProxyCachePathConfig>>,
    location: &SourceLocation,
) -> ConfigResult<()> {
    if let ProxyCacheConfig::Zone(name) = &context.proxy_cache
        && !zones.contains_key(name)
    {
        return Err(ConfigError::at(
            location.clone(),
            format!("cache zone \"{name}\" is not defined"),
        ));
    }
    for (directive, config) in [
        ("fastcgi_cache", &context.fastcgi_cache),
        ("uwsgi_cache", &context.uwsgi_cache),
        ("scgi_cache", &context.scgi_cache),
    ] {
        if let ProxyCacheConfig::Zone(name) = &config.cache
            && !zones.contains_key(name)
        {
            return Err(ConfigError::at(
                location.clone(),
                format!("\"{directive}\" zone \"{name}\" is unknown"),
            ));
        }
    }
    Ok(())
}
