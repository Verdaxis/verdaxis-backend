"""Install as sitecustomize in fixture subprocesses. Deny non-fixture egress."""
import os
import sys


def guard(event, arguments):
    if event == 'socket.connect':
        _socket, address = arguments
        if not isinstance(address, tuple) or address[:2] != ('127.0.0.1', int(os.environ['PHASE0_PORT'])):
            raise PermissionError('Phase 0 permits only its disposable PostgreSQL endpoint')
    elif event == 'socket.getaddrinfo':
        host, port, *_ = arguments
        if host != '127.0.0.1' or int(port) != int(os.environ['PHASE0_PORT']):
            raise PermissionError('Phase 0 denies external DNS and network access')


sys.addaudithook(guard)
