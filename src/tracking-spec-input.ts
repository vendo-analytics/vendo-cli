import {
  TRACKING_SPEC_VALUE_TYPES,
  type CreateTrackingSpecInput,
  type TrackingSpecBindingInput,
  type TrackingSpecPayloadInput,
  type TrackingSpecValueType,
  type UpdateTrackingSpecInput,
} from './api/tracking-specs.js';

type JsonObject = Record<string, unknown>;

interface CreateOverrides {
  name?: string;
  description?: string;
  strategyId?: string | null;
}

interface UpdateOverrides {
  name?: string;
  description?: string;
  expectedRevision: number;
}

interface SpecFileWrapper {
  name?: string;
  description?: string;
  strategyId?: string | null;
  payload?: TrackingSpecPayloadInput;
  bindings?: TrackingSpecBindingInput[];
}

const valueTypes = new Set<string>(TRACKING_SPEC_VALUE_TYPES);

/**
 * Accept either the API wrapper or a bare payload document. This keeps a spec
 * file useful in Documents and source control without forcing the API's
 * container metadata into it.
 */
export function normalizeCreateTrackingSpecInput(
  document: unknown | undefined,
  overrides: CreateOverrides = {},
): CreateTrackingSpecInput {
  const file = parseSpecFile(document);
  const name = overrides.name ?? file.name;

  if (!name || name.trim().length === 0) {
    throw new Error(
      'Spec name is required. Pass --name or include "name" in the JSON file.',
    );
  }

  const body: CreateTrackingSpecInput = { name: name.trim() };
  const description = overrides.description ?? file.description;
  const strategyId = overrides.strategyId ?? file.strategyId;

  if (description !== undefined) body.description = description;
  if (strategyId !== undefined) body.strategyId = strategyId;
  if (file.payload !== undefined) body.payload = file.payload;
  if (file.bindings !== undefined) body.bindings = file.bindings;

  return body;
}

export function normalizeUpdateTrackingSpecInput(
  document: unknown | undefined,
  overrides: UpdateOverrides,
): UpdateTrackingSpecInput {
  assertRevision(overrides.expectedRevision);
  const file = parseSpecFile(document);
  if (file.strategyId !== undefined) {
    throw new Error(
      'strategyId cannot be changed by specs update; remove it from the JSON file.',
    );
  }
  const body: UpdateTrackingSpecInput = {
    expectedRevision: overrides.expectedRevision,
  };

  const name = overrides.name ?? file.name;
  const description = overrides.description ?? file.description;

  if (name !== undefined) {
    if (name.trim().length === 0) {
      throw new Error('Spec name cannot be empty.');
    }
    body.name = name.trim();
  }
  if (description !== undefined) body.description = description;
  if (file.payload !== undefined) body.payload = file.payload;
  if (file.bindings !== undefined) body.bindings = file.bindings;

  if (Object.keys(body).length === 1) {
    throw new Error(
      'Nothing to update. Pass --name, --description, or a JSON file with payload/bindings.',
    );
  }

  return body;
}

export function parsePositiveInteger(
  value: string | number,
  flagName: string,
): number {
  const parsed = typeof value === 'number' ? value : Number(value);
  if (!Number.isInteger(parsed) || parsed < 1) {
    throw new Error(`${flagName} must be a positive integer.`);
  }
  return parsed;
}

export function parseNonNegativeInteger(
  value: string | number,
  flagName: string,
): number {
  const parsed = typeof value === 'number' ? value : Number(value);
  if (!Number.isInteger(parsed) || parsed < 0) {
    throw new Error(`${flagName} must be a non-negative integer.`);
  }
  return parsed;
}

function parseSpecFile(document: unknown | undefined): SpecFileWrapper {
  if (document === undefined) return {};

  const root = asObject(document, 'Spec file root');
  const isPayload =
    'events' in root ||
    'userProperties' in root ||
    'groupProperties' in root ||
    Object.keys(root).length === 0;

  if (isPayload) {
    const payload = validatePayload(root);
    return { payload };
  }

  const allowedWrapperKeys = new Set([
    'name',
    'description',
    'strategyId',
    'payload',
    'bindings',
    'expectedRevision',
  ]);
  const hasWrapperKey = Object.keys(root).some((key) =>
    allowedWrapperKeys.has(key),
  );
  if (!hasWrapperKey) {
    throw new Error(
      'Spec file must be a payload ({ events, userProperties, groupProperties }) or an API wrapper containing "payload".',
    );
  }

  const result: SpecFileWrapper = {};
  if (root.name !== undefined) {
    result.name = asString(root.name, 'name');
  }
  if (root.description !== undefined) {
    result.description = asString(root.description, 'description');
  }
  if (root.strategyId !== undefined) {
    result.strategyId =
      root.strategyId === null ? null : asString(root.strategyId, 'strategyId');
  }
  if (root.payload !== undefined) {
    result.payload = validatePayload(asObject(root.payload, 'payload'));
  }
  if (root.bindings !== undefined) {
    result.bindings = validateBindings(root.bindings);
  }
  return result;
}

function validatePayload(value: JsonObject): TrackingSpecPayloadInput {
  const payload: TrackingSpecPayloadInput = {};

  if (value.events !== undefined) {
    payload.events = asArray(value.events, 'events').map((event, index) => {
      const row = asObject(event, `events[${index}]`);
      validateStableKey(row.stableKey, `events[${index}].stableKey`);
      requiredString(row.name, `events[${index}].name`);
      optionalString(row.description, `events[${index}].description`);
      optionalEnum(
        row.category,
        ['acquisition', 'engagement', 'conversion', 'retention'],
        `events[${index}].category`,
      );
      optionalEnum(
        row.source,
        ['web', 'ios', 'android', 'server', 'third_party'],
        `events[${index}].source`,
        true,
      );
      optionalEnum(
        row.specStatus,
        ['planned', 'implemented', 'validated', 'deprecated'],
        `events[${index}].specStatus`,
      );
      optionalString(
        row.triggerDescription,
        `events[${index}].triggerDescription`,
        true,
      );
      optionalString(
        row.analyticsRequirement,
        `events[${index}].analyticsRequirement`,
        true,
      );
      if (row.properties !== undefined) {
        asArray(row.properties, `events[${index}].properties`).forEach(
          (property, propertyIndex) => {
            validateEventProperty(
              property,
              `events[${index}].properties[${propertyIndex}]`,
            );
          },
        );
      }
      if (row.destinations !== undefined) {
        validateStringArray(row.destinations, `events[${index}].destinations`);
      }
      return row as unknown as NonNullable<
        TrackingSpecPayloadInput['events']
      >[number];
    });
  }

  if (value.userProperties !== undefined) {
    payload.userProperties = asArray(
      value.userProperties,
      'userProperties',
    ).map((property, index) => {
      const path = `userProperties[${index}]`;
      const row = asObject(property, path);
      validateStandaloneProperty(row, path);
      optionalBoolean(row.identify, `${path}.identify`);
      optionalBoolean(row.setOnce, `${path}.setOnce`);
      optionalString(row.bqLocation, `${path}.bqLocation`, true);
      return row as unknown as NonNullable<
        TrackingSpecPayloadInput['userProperties']
      >[number];
    });
  }

  if (value.groupProperties !== undefined) {
    payload.groupProperties = asArray(
      value.groupProperties,
      'groupProperties',
    ).map((property, index) => {
      const path = `groupProperties[${index}]`;
      const row = asObject(property, path);
      validateStandaloneProperty(row, path);
      requiredString(row.groupType, `${path}.groupType`);
      optionalBoolean(row.isGroupKey, `${path}.isGroupKey`);
      if (row.sources !== undefined) {
        validateStringArray(row.sources, `${path}.sources`);
      }
      return row as unknown as NonNullable<
        TrackingSpecPayloadInput['groupProperties']
      >[number];
    });
  }

  return payload;
}

function validateEventProperty(value: unknown, path: string): void {
  const property = asObject(value, path);
  validateStableKey(property.stableKey, `${path}.stableKey`);
  requiredString(property.name, `${path}.name`);
  const type = requiredValueType(property.type, `${path}.type`);
  optionalString(property.description, `${path}.description`);
  optionalBoolean(property.required, `${path}.required`);
  validateAllowedValues(property.allowedValues, type, `${path}.allowedValues`);
}

function validateStandaloneProperty(property: JsonObject, path: string): void {
  validateStableKey(property.stableKey, `${path}.stableKey`);
  requiredString(property.name, `${path}.name`);
  optionalString(property.definition, `${path}.definition`);
  optionalEnum(
    property.specStatus,
    ['planned', 'implemented', 'validated', 'deprecated'],
    `${path}.specStatus`,
  );
  const type =
    property.dataType === undefined
      ? 'string'
      : requiredValueType(property.dataType, `${path}.dataType`);
  optionalBoolean(property.required, `${path}.required`);
  validateAllowedValues(property.allowedValues, type, `${path}.allowedValues`);
}

function validateAllowedValues(
  value: unknown,
  type: TrackingSpecValueType,
  path: string,
): void {
  if (value === undefined) return;
  const values = asArray(value, path);

  if (values.length > 0 && (type === 'array' || type === 'object')) {
    throw new Error(`${path} is not supported for ${type} properties.`);
  }

  const expected = type === 'datetime' ? 'string' : type;
  for (const [index, item] of values.entries()) {
    if (
      !['string', 'number', 'boolean'].includes(typeof item) ||
      typeof item !== expected
    ) {
      throw new Error(`${path}[${index}] must be a ${expected}.`);
    }
  }
}

function validateBindings(value: unknown): TrackingSpecBindingInput[] {
  return asArray(value, 'bindings').map((binding, index) => {
    const path = `bindings[${index}]`;
    const row = asObject(binding, path);
    const kind = requiredString(row.kind, `${path}.kind`);

    optionalString(row.displayName, `${path}.displayName`);
    if (row.validationConfig !== undefined) {
      asObject(row.validationConfig, `${path}.validationConfig`);
    }

    if (kind === 'tracking_write_key') {
      const id = requiredString(
        row.trackingWriteKeyId,
        `${path}.trackingWriteKeyId`,
      );
      validateUuid(id, `${path}.trackingWriteKeyId`);
      if (row.sourceSemanticMappingId !== undefined) {
        throw new Error(
          `${path} cannot include sourceSemanticMappingId for a tracking_write_key binding.`,
        );
      }
      if (row.environment !== undefined) {
        throw new Error(
          `${path}.environment must not be supplied for a tracking_write_key binding; it comes from the write key.`,
        );
      }
      return row as unknown as TrackingSpecBindingInput;
    }

    if (kind === 'semantic_mapping') {
      const id = requiredString(
        row.sourceSemanticMappingId,
        `${path}.sourceSemanticMappingId`,
      );
      validateUuid(id, `${path}.sourceSemanticMappingId`);
      if (row.trackingWriteKeyId !== undefined) {
        throw new Error(
          `${path} cannot include trackingWriteKeyId for a semantic_mapping binding.`,
        );
      }
      const environment = requiredString(
        row.environment,
        `${path}.environment`,
      );
      if (!['production', 'staging', 'development'].includes(environment)) {
        throw new Error(
          `${path}.environment must be production, staging, or development.`,
        );
      }
      return row as unknown as TrackingSpecBindingInput;
    }

    throw new Error(
      `${path}.kind must be tracking_write_key or semantic_mapping.`,
    );
  });
}

function requiredValueType(
  value: unknown,
  path: string,
): TrackingSpecValueType {
  const type = requiredString(value, path);
  if (!valueTypes.has(type)) {
    throw new Error(
      `${path} must be one of: ${TRACKING_SPEC_VALUE_TYPES.join(', ')}.`,
    );
  }
  return type as TrackingSpecValueType;
}

function validateStableKey(value: unknown, path: string): void {
  if (value === undefined) return;
  const stableKey = requiredString(value, path);
  validateUuid(stableKey, path);
}

function validateUuid(value: string, path: string): void {
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(
      value,
    )
  ) {
    throw new Error(`${path} must be a UUID.`);
  }
}

function validateStringArray(value: unknown, path: string): void {
  asArray(value, path).forEach((item, index) => {
    requiredString(item, `${path}[${index}]`);
  });
}

function optionalBoolean(value: unknown, path: string): void {
  if (value !== undefined && typeof value !== 'boolean') {
    throw new Error(`${path} must be a boolean.`);
  }
}

function optionalString(value: unknown, path: string, nullable = false): void {
  if (value === undefined || (nullable && value === null)) return;
  asString(value, path);
}

function optionalEnum(
  value: unknown,
  values: string[],
  path: string,
  nullable = false,
): void {
  if (value === undefined || (nullable && value === null)) return;
  const string = asString(value, path);
  if (!values.includes(string)) {
    throw new Error(`${path} must be one of: ${values.join(', ')}.`);
  }
}

function requiredString(value: unknown, path: string): string {
  const string = asString(value, path);
  if (string.trim().length === 0) {
    throw new Error(`${path} cannot be empty.`);
  }
  return string;
}

function asString(value: unknown, path: string): string {
  if (typeof value !== 'string') {
    throw new Error(`${path} must be a string.`);
  }
  return value;
}

function asArray(value: unknown, path: string): unknown[] {
  if (!Array.isArray(value)) {
    throw new Error(`${path} must be an array.`);
  }
  return value;
}

function asObject(value: unknown, path: string): JsonObject {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${path} must be a JSON object.`);
  }
  return value as JsonObject;
}

function assertRevision(revision: number): void {
  if (!Number.isInteger(revision) || revision < 0) {
    throw new Error('expectedRevision must be a non-negative integer.');
  }
}
