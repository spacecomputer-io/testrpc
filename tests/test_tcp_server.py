#!/usr/bin/env python3
"""
Simple TCP server for testing Autobahn transactions.
Accepts length-delimited binary messages and logs them.
"""

import asyncio
import struct
import logging

logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')

class AutobahnTestServer:
    def __init__(self, port):
        self.port = port
        self.tx_count = 0
    
    async def handle_client(self, reader, writer):
        client_addr = writer.get_extra_info('peername')
        logging.info(f"Client connected from {client_addr}")
        
        try:
            while True:
                # Read length prefix (4 bytes, big-endian)
                length_bytes = await reader.read(4)
                if not length_bytes:
                    break
                    
                if len(length_bytes) < 4:
                    logging.warning(f"Incomplete length prefix from {client_addr}")
                    break
                
                length = struct.unpack('>I', length_bytes)[0]
                logging.info(f"Expecting transaction of {length} bytes")
                
                # Read the transaction data
                tx_data = await reader.read(length)
                if len(tx_data) != length:
                    logging.warning(f"Incomplete transaction data: expected {length}, got {len(tx_data)}")
                    break
                
                # Parse transaction (first byte indicates type, next 8 bytes are ID)
                if len(tx_data) >= 9:
                    tx_type = tx_data[0]
                    tx_id = struct.unpack('>Q', tx_data[1:9])[0]
                    
                    self.tx_count += 1
                    
                    if tx_type == 0:
                        logging.info(f"Received SAMPLE transaction #{self.tx_count}: ID={tx_id}, size={len(tx_data)} bytes")
                    elif tx_type == 1:
                        logging.info(f"Received STANDARD transaction #{self.tx_count}: ID={tx_id}, size={len(tx_data)} bytes")
                    else:
                        logging.info(f"Received UNKNOWN transaction #{self.tx_count}: type={tx_type}, ID={tx_id}, size={len(tx_data)} bytes")
                else:
                    logging.warning(f"Transaction too short: {len(tx_data)} bytes")
                    
        except Exception as e:
            logging.error(f"Error handling client {client_addr}: {e}")
        finally:
            writer.close()
            await writer.wait_closed()
            logging.info(f"Client {client_addr} disconnected. Total transactions received: {self.tx_count}")

    async def start_server(self):
        server = await asyncio.start_server(
            self.handle_client,
            '127.0.0.1',
            self.port
        )
        
        addr = server.sockets[0].getsockname()
        logging.info(f"Autobahn test server running on {addr[0]}:{addr[1]}")
        
        async with server:
            await server.serve_forever()

async def main():
    # Start servers on ports 4000-4003 to match autobahn-nodes.json
    servers = []
    for port in [4000, 4001, 4002, 4003]:
        server = AutobahnTestServer(port)
        servers.append(asyncio.create_task(server.start_server()))
    
    try:
        await asyncio.gather(*servers)
    except KeyboardInterrupt:
        logging.info("Shutting down servers...")

if __name__ == "__main__":
    asyncio.run(main()) 