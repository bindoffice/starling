use super::*;

mod directives;
mod finalize;
mod state;
mod validation;

use state::HttpCompileState;

pub(super) fn compile_http<'a>(
    directive: &'a Directive,
    prefix: &'a Path,
) -> ConfigResult<HttpConfig> {
    let mut state = HttpCompileState::new(prefix);
    for child in &directive.children {
        state.apply(child)?;
    }
    state.finish(directive)
}
