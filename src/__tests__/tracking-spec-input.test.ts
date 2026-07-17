import { describe, expect, it } from 'vitest';

import {
  normalizeCreateTrackingSpecInput,
  normalizeUpdateTrackingSpecInput,
  parseNonNegativeInteger,
  parsePositiveInteger,
} from '../tracking-spec-input.js';

const payload = {
  events: [
    {
      name: 'Integration Completed',
      properties: [
        {
          name: 'integration_name',
          type: 'string',
          required: true,
          allowedValues: ['shopify', 'stripe'],
        },
      ],
    },
  ],
  userProperties: [
    {
      name: 'subscription_status',
      dataType: 'string',
      allowedValues: ['trial', 'active'],
    },
  ],
  groupProperties: [
    {
      groupType: 'company',
      name: 'integration_count',
      dataType: 'number',
    },
  ],
};

describe('tracking spec file normalization', () => {
  it('wraps a bare payload and allows stable keys to be omitted', () => {
    expect(
      normalizeCreateTrackingSpecInput(payload, { name: 'Lifecycle' }),
    ).toEqual({
      name: 'Lifecycle',
      payload,
    });
  });

  it('preserves wrapper bindings and lets CLI metadata override the file', () => {
    const bindings = [
      {
        kind: 'semantic_mapping',
        environment: 'production',
        sourceSemanticMappingId: '5e847d06-f04d-48df-b946-c0bb2c90a661',
        validationConfig: { sampleWindowDays: 7 },
      },
    ];

    expect(
      normalizeCreateTrackingSpecInput(
        {
          name: 'From file',
          description: 'File description',
          strategyId: 'strategy-1',
          payload,
          bindings,
        },
        {
          name: 'From flag',
          description: 'Flag description',
        },
      ),
    ).toEqual({
      name: 'From flag',
      description: 'Flag description',
      strategyId: 'strategy-1',
      payload,
      bindings,
    });
  });

  it('builds an update with the required optimistic revision', () => {
    expect(
      normalizeUpdateTrackingSpecInput(
        {
          payload,
          bindings: [
            {
              kind: 'tracking_write_key',
              trackingWriteKeyId: '9c124d07-c155-477f-8844-36f0a4610f44',
            },
          ],
        },
        { expectedRevision: 3, name: 'Updated lifecycle' },
      ),
    ).toEqual({
      expectedRevision: 3,
      name: 'Updated lifecycle',
      payload,
      bindings: [
        {
          kind: 'tracking_write_key',
          trackingWriteKeyId: '9c124d07-c155-477f-8844-36f0a4610f44',
        },
      ],
    });
  });

  it('rejects group properties without a groupType', () => {
    expect(() =>
      normalizeCreateTrackingSpecInput(
        {
          groupProperties: [
            {
              name: 'plan',
              dataType: 'string',
            },
          ],
        },
        { name: 'Invalid' },
      ),
    ).toThrow(/groupType/);
  });

  it('rejects allowed values that do not match the declared type', () => {
    expect(() =>
      normalizeCreateTrackingSpecInput(
        {
          userProperties: [
            {
              name: 'integration_count',
              dataType: 'number',
              allowedValues: ['many'],
            },
          ],
        },
        { name: 'Invalid' },
      ),
    ).toThrow(/must be a number/);
  });

  it('enforces the binding target union and environment ownership', () => {
    expect(() =>
      normalizeCreateTrackingSpecInput(
        {
          payload,
          bindings: [
            {
              kind: 'tracking_write_key',
              trackingWriteKeyId: '9c124d07-c155-477f-8844-36f0a4610f44',
              environment: 'production',
            },
          ],
        },
        { name: 'Invalid binding' },
      ),
    ).toThrow(/must not be supplied/);

    expect(() =>
      normalizeCreateTrackingSpecInput(
        {
          payload,
          bindings: [
            {
              kind: 'semantic_mapping',
              sourceSemanticMappingId: '5e847d06-f04d-48df-b946-c0bb2c90a661',
              trackingWriteKeyId: '9c124d07-c155-477f-8844-36f0a4610f44',
              environment: 'staging',
            },
          ],
        },
        { name: 'Invalid binding' },
      ),
    ).toThrow(/cannot include trackingWriteKeyId/);
  });

  it('requires a name and at least one update field', () => {
    expect(() => normalizeCreateTrackingSpecInput(payload)).toThrow(/name/i);
    expect(() =>
      normalizeUpdateTrackingSpecInput(undefined, { expectedRevision: 1 }),
    ).toThrow(/Nothing to update/);
  });

  it('does not silently discard immutable strategy linkage on update', () => {
    expect(() =>
      normalizeUpdateTrackingSpecInput(
        {
          strategyId: '4e29c1bd-0941-48c3-b744-cea3a2089f18',
          payload,
        },
        { expectedRevision: 2 },
      ),
    ).toThrow(/strategyId cannot be changed/);
  });
});

describe('parsePositiveInteger', () => {
  it('accepts positive integer versions and revisions', () => {
    expect(parsePositiveInteger('12', '--revision')).toBe(12);
  });

  it('rejects zero, decimals, and non-numeric values', () => {
    expect(() => parsePositiveInteger('0', '--revision')).toThrow(
      /positive integer/,
    );
    expect(() => parsePositiveInteger('1.5', '--revision')).toThrow(
      /positive integer/,
    );
    expect(() => parsePositiveInteger('nope', '--revision')).toThrow(
      /positive integer/,
    );
  });
});

describe('parseNonNegativeInteger', () => {
  it('accepts revision zero for pre-existing drafts', () => {
    expect(parseNonNegativeInteger('0', '--revision')).toBe(0);
  });

  it('rejects negative and fractional revisions', () => {
    expect(() => parseNonNegativeInteger('-1', '--revision')).toThrow(
      /non-negative integer/,
    );
    expect(() => parseNonNegativeInteger('2.5', '--revision')).toThrow(
      /non-negative integer/,
    );
  });
});
