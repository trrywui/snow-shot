"""Generate Full/Mini MCP catalogs and the application's recording input contract.

Edit mcp-capabilities.source.json. Reviewed adapter mappings remain independent
of the Qt/Rust parity checks; recording primitives have one canonical contract
consumed by Qt and compared with compiled Rust schemas and deserialization.
"""

import argparse
import copy
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MANIFEST = 'mcp-capabilities.source.json'
RECORDING_HEADER = 'include/snow_shot/app/mcp/recordingoptioncontracts.h'


def string_list(value, context):
    if (not isinstance(value, list) or not all(isinstance(item, str) for item in value) or
            len(value) != len(set(value))):
        raise ValueError(f'{context} must be a list of unique strings')
    return value


def recording_validation(manifest):
    recording = manifest['catalog']['surface_contracts']['recording_options']
    if set(recording) != {'adapter', 'validation'}:
        raise ValueError('Recording members derive from validation; unknown recording fields')
    validation = recording['validation']
    if not isinstance(validation, dict) or not validation:
        raise ValueError('Recording validation must be a nonempty object')
    for name, contract in validation.items():
        if (not isinstance(contract, dict) or
                not set(contract) <= {'type', 'minimum', 'maximum', 'enum', 'format'}):
            raise ValueError(f'Unknown recording constraint fields: {name}')
        kind = contract.get('type')
        if kind not in {'string', 'integer', 'boolean'}:
            raise ValueError(f'Unsupported recording type: {name}')
        if 'format' in contract and (kind != 'string' or
                                     contract['format'] not in {'qt-color', 'output-path'}):
            raise ValueError(f'Unsupported recording format: {name}')
        numeric = {'minimum', 'maximum'} & set(contract)
        if numeric:
            if (kind != 'integer' or numeric != {'minimum', 'maximum'} or
                    any(type(contract[key]) is not int for key in numeric) or
                    contract['minimum'] > contract['maximum']):
                raise ValueError(f'Invalid recording bounds: {name}')
        if 'enum' in contract:
            choices = contract['enum']
            expected_type = {'integer': int, 'string': str, 'boolean': bool}[kind]
            if (not isinstance(choices, list) or not choices or
                    any(type(choice) is not expected_type for choice in choices) or
                    len(choices) != len(set(choices))):
                raise ValueError(f'Invalid recording choices: {name}')
            if numeric and not all(contract['minimum'] <= choice <= contract['maximum']
                                   for choice in choices):
                raise ValueError(f'Recording choices outside bounds: {name}')
        if kind == 'integer' and not numeric and 'enum' not in contract:
            raise ValueError(f'Recording integer needs bounds or choices: {name}')
    return validation


def load_manifest(root=ROOT):
    def unique_object(pairs):
        result = {}
        for name, value in pairs:
            if name in result:
                raise ValueError(f'Duplicate capability manifest key: {name}')
            result[name] = value
        return result

    return json.loads((root / MANIFEST).read_text(encoding='utf-8'),
                      object_pairs_hook=unique_object)


def catalogs(manifest):
    if set(manifest) != {'catalog', 'mini'}:
        raise ValueError('Capability manifest must contain catalog and mini')
    validation = recording_validation(manifest)
    full = copy.deepcopy(manifest['catalog'])
    full['surface_contracts']['recording_options'] = {
        'adapter': full['surface_contracts']['recording_options']['adapter'],
        'members': list(validation),
        'validation': copy.deepcopy(validation),
    }
    mini = copy.deepcopy(full)
    policy = manifest['mini']
    if set(policy) != {'disabled_tools', 'domain_scopes', 'validation', 'surface_contracts',
                       'product', 'recognition_modes'}:
        raise ValueError('Unknown or missing Mini capability policy fields')
    disabled = set(string_list(policy['disabled_tools'], 'Mini disabled tools'))
    declared = {tool for domain in full['domains'].values() for tool in domain['tools']}
    if not disabled <= declared:
        raise ValueError('Mini disabled tools must be members of the Full catalog')
    for domain in mini['domains'].values():
        domain['tools'] = [tool for tool in domain['tools'] if tool not in disabled]
    for name, scope in policy['domain_scopes'].items():
        mini['domains'][name]['scope'] = scope
    mini['validation'].update(policy['validation'])
    for name, override in policy['surface_contracts'].items():
        if not set(override) <= {'adapter', 'exclude_members'}:
            raise ValueError(f'Unknown Mini surface override fields: {name}')
        contract = mini['surface_contracts'][name]
        excluded = string_list(override.get('exclude_members', []), f'Mini exclusions: {name}')
        if not set(excluded) <= set(contract['members']):
            raise ValueError(f'Mini exclusions must be Full members: {name}')
        if excluded:
            contract['members'] = [member for member in contract['members'] if member not in excluded]
        if 'adapter' in override:
            contract['adapter'] = override['adapter']
    mini['product'] = policy['product']
    mini['recognition_modes'] = policy['recognition_modes']
    return {'mcp-capabilities.json': full, 'mcp-capabilities-mini.json': mini}


def cpp_value(value):
    if isinstance(value, str):
        return 'QStringLiteral(' + json.dumps(value) + ')'
    return json.dumps(value)


def recording_header(validation):
    lines = [
        '// Generated by scripts/generate_mcp_capabilities.py; edit mcp-capabilities.source.json.',
        '#ifndef SNOW_SHOT_APP_MCP_RECORDINGOPTIONCONTRACTS_H',
        '#define SNOW_SHOT_APP_MCP_RECORDINGOPTIONCONTRACTS_H',
        '', '#include <QJsonArray>', '#include <QJsonObject>', '',
        'namespace snow_shot::app::mcp {',
        'inline const QJsonObject& recordingAutomationOptionContracts() {',
        '    // clang-format off',
        '    static const QJsonObject contracts{',
    ]
    for name, contract in validation.items():
        lines.append(f'        {{{cpp_value(name)},')
        lines.append('         QJsonObject{')
        entries = list(contract.items())
        for index, (key, value) in enumerate(entries):
            suffix = ',' if index < len(entries) - 1 else '}},'
            if isinstance(value, list):
                lines.append(f'             {{{cpp_value(key)}, QJsonArray{{')
                for choice_index, choice in enumerate(value):
                    ending = ',' if choice_index < len(value) - 1 else '}}' + suffix
                    lines.append('                 ' + cpp_value(choice) + ending)
            else:
                lines.append(f'             {{{cpp_value(key)}, {cpp_value(value)}}}{suffix}')
    lines.extend(['    };', '    // clang-format on', '    return contracts;', '}',
                  '} // namespace snow_shot::app::mcp', '',
                  '#endif // SNOW_SHOT_APP_MCP_RECORDINGOPTIONCONTRACTS_H', ''])
    return '\n'.join(lines)


def generated_outputs(root=ROOT):
    manifest = load_manifest(root)
    outputs = {name: json.dumps(catalog, ensure_ascii=False, indent=2) + '\n'
               for name, catalog in catalogs(manifest).items()}
    outputs[RECORDING_HEADER] = recording_header(recording_validation(manifest))
    return outputs


def check_generated(root=ROOT):
    for name, expected in generated_outputs(root).items():
        if not (root / name).is_file() or (root / name).read_text(encoding='utf-8') != expected:
            raise ValueError(f'{name} is stale; run python snow_shot/scripts/generate_mcp_capabilities.py')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='Reject stale generated outputs')
    args = parser.parse_args()
    if args.check:
        check_generated()
    else:
        for name, contents in generated_outputs().items():
            path = ROOT / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(contents, encoding='utf-8', newline='\n')


if __name__ == '__main__':
    main()
