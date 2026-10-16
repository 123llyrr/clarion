#!/usr/bin/env python3
"""Rebuild embedded Vulkan animation shaders with glslc (shaderc)."""
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parent.parent / 'src/render_helpers'
output = root / 'vulkan/shaders'


def translate(source):
    lines = ['#version 450', 'layout(location=0) in vec2 clarion_v_coords;',
             'layout(location=0) out vec4 clarion_out;', '#define gl_FragColor clarion_out',
             '#define texture2D texture']
    slots = samplers = 0
    for line in source.splitlines():
        text = line.strip()
        if text.startswith(('precision ', 'varying ')):
            continue
        if not text.startswith('uniform '):
            lines.append(line)
            continue
        _, kind, name = text.rstrip(';').split()
        if kind == 'sampler2D':
            lines.append(f'layout(set=0,binding={samplers}) uniform sampler2D {name};')
            samplers += 1
            continue
        if kind == 'mat3':
            expr = 'mat3(' + ','.join(f'u.data[{slots+c}].xyz' for c in range(3)) + ')'
            count = 3
        else:
            suffix = {'float': '.x', 'vec2': '.xy', 'vec3': '.xyz', 'vec4': ''}[kind]
            expr = f'u.data[{slots}]{suffix}'
            count = 1
        lines.append(f'#define {name} ({expr})')
        slots += count
    lines.insert(1, f'layout(std140,set=0,binding=2) uniform Data {{ vec4 data[{max(slots,1)}]; }} u;')
    return '\n'.join(lines) + '\n'


def compile_shader(source, stage, name):
    subprocess.run(['glslc', '--target-env=vulkan1.2', f'-fshader-stage={stage}',
                    '-o', str(output / (name + '.spv')), '-'], input=source, text=True, check=True)


compile_shader((output / 'animation.vert').read_text(), 'vert', 'animation.vert')
for stage in ['vert', 'frag']:
    name = 'particle.' + stage
    compile_shader((output / name).read_text(), stage, name)
for name, kind, body in [('resize', 'resize', 'resize'), ('open', 'open', 'open_default'),
                         ('close', 'close', 'close_default'),
                         ('explosion', 'close', 'denial_explosion'),
                         ('implode', 'close', 'denial_implode')]:
    files = [kind + '_prelude', body, kind + '_epilogue']
    if kind == 'resize':
        files.append('rounding_alpha')
    source = ''.join((root / 'shaders' / (file + '.frag')).read_text() for file in files)
    compile_shader(translate(source), 'frag', 'animation_' + name + '.frag')
