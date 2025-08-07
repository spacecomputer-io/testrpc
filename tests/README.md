# Test Configurations

This directory contains test configurations for the testrpc tool.

## Files

### `autobahn-authorities.yaml`

Test configuration for the Autobahn adapter using the authorities JSON format.

- Uses localhost endpoints for safe testing
- Includes multiple rounds with different transaction patterns
- Points to `autobahn-authorities.json` for node configuration

### `autobahn-authorities.json`

Mock Autobahn authorities configuration with localhost endpoints.

- Contains 4 authorities with worker configurations
- All transaction endpoints point to `127.0.0.1:4000-4003`
- Safe for local testing when used with `test_tcp_server.py`

## Running Tests

1. **Dry run (recommended first):**

   ```bash
   cargo run -- -f tests/autobahn-authorities.yaml --dry-run
   ```

2. **With mock TCP servers:**

   ```bash
   # Terminal 1: Start mock servers
   python3 test_tcp_server.py

   # Terminal 2: Run test
   cargo run -- -f tests/autobahn-authorities.yaml
   ```

3. **With real Autobahn network:**
   - Update `autobahn-authorities.json` with real node endpoints
   - Run without dry-run flag
