"""One-shot scalar bindings at confirmed exits; never retain execution owners."""


def primitives(value, depth=0):
    if depth > 12:
        raise ValueError('binding metadata depth exceeded')
    if value is None or type(value) in (str, int, bool, float):
        return value
    if type(value) in (tuple, list):
        if len(value) > 4096:
            raise ValueError('binding metadata length exceeded')
        return [primitives(item, depth + 1) for item in value]
    if type(value) is dict and all(type(key) in (str, int) for key in value):
        return {str(key): primitives(item, depth + 1) for key, item in value.items()}
    raise TypeError('nonprimitive binding metadata')


def tensor_ranges(value, path=()):
    import torch
    if type(value) is torch.Tensor:
        return [{'path': list(path), 'address': value.data_ptr(),
                 'logical_bytes': value.numel() * value.element_size(),
                 'storage_offset': value.storage_offset(), 'shape': list(value.shape),
                 'stride': list(value.stride()), 'dtype': str(value.dtype)}]
    if type(value) in (list, tuple):
        result = []
        for index, item in enumerate(value):
            result.extend(tensor_ranges(item, (*path, index)))
        return result
    if type(value) is dict:
        result = []
        for key, item in value.items():
            if type(key) not in (str, int):
                raise TypeError('unsupported binding key')
            result.extend(tensor_ranges(item, (*path, key)))
        return result
    if value is None or type(value) in (str, int, float, bool):
        return []
    raise TypeError('unsupported binding leaf')


def definition_binding(segment):
    definition = segment._definition
    return {'available': True, 'plan_handle': definition.plan._handle,
            'pool': list(definition.pool),
            'python_graph_object_id': id(segment._graph),
            'streams': [stream.cuda_stream for stream in definition.streams],
            'scope': 'sampled metadata, not physical residency/completion',
            'static_inputs': tensor_ranges(definition.static_tree),
            'static_outputs': tensor_ranges(definition.outputs),
            'buffers': tensor_ranges(definition.buffer_owners),
            'footprint': primitives(definition.footprint.facts())}
