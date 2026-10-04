"""Read PE resources without executing the binary or requiring a Windows host."""
import struct
from pathlib import Path


def resources(path):
    data = Path(path).read_bytes()
    if len(data) < 64 or data[:2] != b'MZ':
        raise ValueError('Not a PE executable')
    pe = struct.unpack_from('<I', data, 60)[0]
    if data[pe:pe + 4] != b'PE\0\0':
        raise ValueError('Missing PE header')
    count = struct.unpack_from('<H', data, pe + 6)[0]
    optional_size = struct.unpack_from('<H', data, pe + 20)[0]
    optional = pe + 24
    magic = struct.unpack_from('<H', data, optional)[0]
    directory = optional + {0x10b: 96, 0x20b: 112}[magic]
    resource_rva, resource_size = struct.unpack_from('<II', data, directory + 16)
    sections = optional + optional_size

    def offset(rva, length=1):
        for index in range(count):
            entry = sections + index * 40
            virtual_size, virtual_address, size, raw = struct.unpack_from('<IIII', data, entry + 8)
            if virtual_address <= rva < virtual_address + max(virtual_size, size):
                result = raw + rva - virtual_address
                if result + length <= len(data) and rva - virtual_address + length <= size:
                    return result
        raise ValueError('PE resource points outside file data')

    if not resource_rva or not resource_size:
        return {}
    base = offset(resource_rva, resource_size)

    def read_directory(relative):
        if relative < 0 or relative + 16 > resource_size:
            raise ValueError('Invalid PE resource directory')
        named, numeric = struct.unpack_from('<HH', data, base + relative + 12)
        entries = named + numeric
        if relative + 16 + entries * 8 > resource_size:
            raise ValueError('Invalid PE resource entries')
        return [struct.unpack_from('<II', data, base + relative + 16 + index * 8) for index in range(entries)]

    def leaves(entry, depth):
        if depth > 3:
            raise ValueError('PE resource nesting is too deep')
        if entry & 0x80000000:
            for _, child in read_directory(entry & 0x7fffffff):
                yield from leaves(child, depth + 1)
        else:
            if entry + 16 > resource_size:
                raise ValueError('Invalid PE resource data entry')
            rva, size = struct.unpack_from('<II', data, base + entry)
            start = offset(rva, size)
            yield data[start:start + size]

    return {kind: list(leaves(entry, 0)) for kind, entry in read_directory(0) if not kind & 0x80000000}


def verify_branding(executable, icon):
    contents = resources(executable)
    if not {3, 14, 16}.issubset(contents):
        raise ValueError('Windows executable is missing icon/group-icon/version resources')
    source = Path(icon).read_bytes()
    reserved, kind, count = struct.unpack_from('<HHH', source)
    if reserved != 0 or kind != 1 or count < 1:
        raise ValueError('Invalid source ICO')
    frames = []
    for index in range(count):
        size, start = struct.unpack_from('<II', source, 6 + index * 16 + 8)
        frames.append(source[start:start + size])
    if not all(frame in contents[3] for frame in frames):
        raise ValueError('Executable icon differs from the OwlWhisp ICO')
    versions = b''.join(contents[16])
    for value in ('CompanyName', 'Fil-Shtopor', 'ProductName', 'OwlWhisp'):
        if value.encode('utf-16-le') not in versions:
            raise ValueError(f'Missing Windows product metadata: {value}')
