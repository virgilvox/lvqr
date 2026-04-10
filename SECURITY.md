# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in LVQR, please report it responsibly.

**Email**: hackbuildvideo@gmail.com

Please include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

## Response Timeline

- **48 hours**: Initial acknowledgment
- **7 days**: Assessment and severity classification
- **30 days**: Fix developed and tested

## Scope

Security issues we care about:
- QUIC/TLS handling and certificate validation
- Authentication token bypass or forgery
- Admin API access control
- Buffer overflow or memory safety issues
- Denial of service via protocol abuse

Out of scope:
- Self-signed certificate warnings in development mode
- Rate limiting on unauthenticated public relays (expected behavior)
