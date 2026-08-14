import ts from 'typescript';

function unwrap(expression: ts.Expression): ts.Expression {
  let current = expression;
  while (
    ts.isParenthesizedExpression(current) ||
    ts.isAsExpression(current) ||
    ts.isTypeAssertionExpression(current) ||
    ts.isNonNullExpression(current)
  ) {
    current = current.expression;
  }
  return current;
}

function staticString(expression: ts.Expression): string | undefined {
  const value = unwrap(expression);
  if (
    ts.isStringLiteral(value) ||
    ts.isNoSubstitutionTemplateLiteral(value)
  ) {
    return value.text;
  }
  if (
    ts.isBinaryExpression(value) &&
    value.operatorToken.kind === ts.SyntaxKind.PlusToken
  ) {
    const left = staticString(value.left);
    const right = staticString(value.right);
    return left === undefined || right === undefined ? undefined : left + right;
  }
  return undefined;
}

function propertyName(expression: ts.Expression): string | undefined {
  const value = unwrap(expression);
  if (ts.isPropertyAccessExpression(value)) return value.name.text;
  if (ts.isElementAccessExpression(value) && value.argumentExpression) {
    return staticString(value.argumentExpression);
  }
  return undefined;
}

function propertyOwner(expression: ts.Expression): ts.Expression | undefined {
  const value = unwrap(expression);
  if (ts.isPropertyAccessExpression(value)) return value.expression;
  if (ts.isElementAccessExpression(value)) return value.expression;
  return undefined;
}

function identifierName(expression: ts.Expression): string | undefined {
  const value = unwrap(expression);
  return ts.isIdentifier(value) ? value.text : undefined;
}

/** Conservatively finds references derived from global Worker constructors. */
export function nestedWorkerConstructionViolations(
  source: string,
  sourceName: string
): string[] {
  const file = ts.createSourceFile(
    sourceName,
    source,
    ts.ScriptTarget.Latest,
    true,
    sourceName.endsWith('.ts') ? ts.ScriptKind.TS : ts.ScriptKind.JS
  );
  const globalAliases = new Set(['globalThis', 'self']);
  const workerAliases = new Set(['Worker', 'SharedWorker']);
  const reflectAliases = new Set(['Reflect']);
  const constructAliases = new Set<string>();

  const isAlias = (expression: ts.Expression, aliases: Set<string>): boolean => {
    const name = identifierName(expression);
    return name !== undefined && aliases.has(name);
  };
  const isWorker = (expression: ts.Expression): boolean => {
    if (isAlias(expression, workerAliases)) return true;
    return (
      (propertyName(expression) === 'Worker' ||
        propertyName(expression) === 'SharedWorker') &&
      propertyOwner(expression) !== undefined &&
      isAlias(propertyOwner(expression)!, globalAliases)
    );
  };
  const isReflect = (expression: ts.Expression): boolean =>
    isAlias(expression, reflectAliases);
  const isReflectConstruct = (expression: ts.Expression): boolean => {
    if (isAlias(expression, constructAliases)) return true;
    return (
      propertyName(expression) === 'construct' &&
      propertyOwner(expression) !== undefined &&
      isReflect(propertyOwner(expression)!)
    );
  };

  const recordAlias = (name: string, expression: ts.Expression): boolean => {
    if (isAlias(expression, globalAliases)) {
      const before = globalAliases.size;
      globalAliases.add(name);
      return globalAliases.size !== before;
    }
    if (isWorker(expression)) {
      const before = workerAliases.size;
      workerAliases.add(name);
      return workerAliases.size !== before;
    }
    if (isReflect(expression)) {
      const before = reflectAliases.size;
      reflectAliases.add(name);
      return reflectAliases.size !== before;
    }
    if (isReflectConstruct(expression)) {
      const before = constructAliases.size;
      constructAliases.add(name);
      return constructAliases.size !== before;
    }
    return false;
  };

  const aliasCandidates: Array<{ name: string; expression: ts.Expression }> = [];
  const collectAliases = (node: ts.Node): void => {
    if (
      ts.isVariableDeclaration(node) &&
      ts.isIdentifier(node.name) &&
      node.initializer
    ) {
      aliasCandidates.push({ name: node.name.text, expression: node.initializer });
    }
    if (
      ts.isBinaryExpression(node) &&
      node.operatorToken.kind === ts.SyntaxKind.EqualsToken &&
      ts.isIdentifier(node.left)
    ) {
      aliasCandidates.push({ name: node.left.text, expression: node.right });
    }
    if (
      ts.isVariableDeclaration(node) &&
      ts.isObjectBindingPattern(node.name) &&
      node.initializer
    ) {
      for (const element of node.name.elements) {
        const importedName = element.propertyName?.getText(file) ?? element.name.getText(file);
        if (
          (importedName === 'Worker' || importedName === 'SharedWorker') &&
          ts.isIdentifier(element.name)
        ) {
          aliasCandidates.push({
            name: element.name.text,
            expression: ts.factory.createPropertyAccessExpression(
              node.initializer,
              importedName
            ),
          });
        }
        if (importedName === 'construct' && ts.isIdentifier(element.name)) {
          aliasCandidates.push({
            name: element.name.text,
            expression: ts.factory.createPropertyAccessExpression(
              node.initializer,
              'construct'
            ),
          });
        }
      }
    }
    ts.forEachChild(node, collectAliases);
  };
  collectAliases(file);
  for (let changed = true; changed; ) {
    changed = false;
    for (const candidate of aliasCandidates) {
      changed = recordAlias(candidate.name, candidate.expression) || changed;
    }
  }

  const isIdentifierReference = (node: ts.Identifier): boolean => {
    const parent = node.parent;
    if (ts.isVariableDeclaration(parent) && parent.name === node) return false;
    if (ts.isBindingElement(parent) && parent.name === node) return false;
    if (ts.isPropertyAccessExpression(parent) && parent.name === node)
      return false;
    if (
      (ts.isPropertyAssignment(parent) ||
        ts.isMethodDeclaration(parent) ||
        ts.isPropertyDeclaration(parent) ||
        ts.isFunctionDeclaration(parent) ||
        ts.isClassDeclaration(parent) ||
        ts.isParameter(parent)) &&
      parent.name === node
    ) {
      return false;
    }
    return true;
  };

  const violations: string[] = [];
  const inspect = (node: ts.Node): void => {
    const detected =
      (ts.isIdentifier(node) &&
        isIdentifierReference(node) &&
        workerAliases.has(node.text)) ||
      ((ts.isPropertyAccessExpression(node) ||
        ts.isElementAccessExpression(node)) &&
        isWorker(node));
    if (detected) {
      const location = file.getLineAndCharacterOfPosition(node.getStart(file));
      violations.push(
        `${sourceName} references a global Worker at ${location.line + 1}:${location.character + 1}`
      );
      return;
    }
    ts.forEachChild(node, inspect);
  };
  inspect(file);
  return violations;
}
