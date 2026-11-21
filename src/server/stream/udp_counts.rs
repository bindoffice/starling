use super::*;

#[derive(Debug)]
pub(in crate::server) struct UdpProxyCounts {
    requests_limit: usize,
    responses_per_request: usize,
    requests: usize,
    responses: usize,
}

impl UdpProxyCounts {
    pub(in crate::server) fn new(requests_limit: usize, responses_per_request: usize) -> Self {
        Self {
            requests_limit,
            responses_per_request,
            requests: 0,
            responses: 0,
        }
    }

    pub(in crate::server) fn record_request(&mut self) {
        self.requests = self.requests.saturating_add(1);
    }

    pub(in crate::server) fn record_response(&mut self) {
        self.responses = self.responses.saturating_add(1);
    }

    pub(in crate::server) fn accepts_requests(&self) -> bool {
        self.requests_limit == 0 || self.requests < self.requests_limit
    }

    pub(in crate::server) fn is_complete(&self) -> bool {
        let requests_done = self.requests_limit == 0 || self.requests >= self.requests_limit;
        if self.requests == 0 || !requests_done {
            return false;
        }
        if self.responses_per_request == STREAM_PROXY_RESPONSES_UNLIMITED {
            return false;
        }
        self.responses >= self.responses_per_request.saturating_mul(self.requests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_request_limits_and_response_multiples() {
        let mut counts = UdpProxyCounts::new(2, 2);
        assert!(counts.accepts_requests());

        counts.record_request();
        assert!(counts.accepts_requests());
        assert!(!counts.is_complete());

        counts.record_response();
        counts.record_response();
        assert!(!counts.is_complete());

        counts.record_request();
        assert!(!counts.accepts_requests());
        assert!(!counts.is_complete());

        counts.record_response();
        counts.record_response();
        assert!(counts.is_complete());
    }

    #[test]
    fn unlimited_responses_wait_for_the_session_timeout() {
        let mut counts = UdpProxyCounts::new(1, STREAM_PROXY_RESPONSES_UNLIMITED);
        counts.record_request();
        assert!(!counts.accepts_requests());
        assert!(!counts.is_complete());
    }

    #[test]
    fn unlimited_requests_complete_after_finite_responses() {
        let mut counts = UdpProxyCounts::new(0, 1);
        counts.record_request();
        assert!(counts.accepts_requests());
        assert!(!counts.is_complete());
        counts.record_response();
        assert!(counts.is_complete());
    }

    #[test]
    fn zero_responses_complete_after_the_request_limit() {
        let mut counts = UdpProxyCounts::new(1, 0);
        assert!(!counts.is_complete());
        counts.record_request();
        assert!(counts.is_complete());
    }
}
