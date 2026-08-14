import ts from 'typescript';

function propertyText(name: ts.PropertyName, file: ts.SourceFile): string {
  if (
    ts.isIdentifier(name) ||
    ts.isStringLiteral(name) ||
    ts.isNumericLiteral(name)
  ) {
    return name.text;
  }
  return name.getText(file);
}

/** Extracts the exact function keys wired to the generated WASM import module. */
export function wasmBindgenGlueImportNames(source: string): Set<string> {
  const file = ts.createSourceFile(
    'cache_wasm.js',
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.JS
  );
  const objectBindings = new Map<string, ts.ObjectLiteralExpression>();
  const collectBindings = (node: ts.Node): void => {
    if (
      ts.isVariableDeclaration(node) &&
      ts.isIdentifier(node.name) &&
      node.initializer &&
      ts.isObjectLiteralExpression(node.initializer)
    ) {
      objectBindings.set(node.name.text, node.initializer);
    }
    ts.forEachChild(node, collectBindings);
  };
  collectBindings(file);

  let importObject: ts.ObjectLiteralExpression | undefined;
  const findImportObject = (node: ts.Node): void => {
    if (
      ts.isPropertyAssignment(node) &&
      propertyText(node.name, file) === './cache_wasm_bg.js'
    ) {
      if (ts.isObjectLiteralExpression(node.initializer)) {
        importObject = node.initializer;
      } else if (ts.isIdentifier(node.initializer)) {
        importObject = objectBindings.get(node.initializer.text);
      }
    }
    ts.forEachChild(node, findImportObject);
  };
  findImportObject(file);
  if (!importObject) {
    throw new Error(
      'generated glue does not define the ./cache_wasm_bg.js import object'
    );
  }
  return new Set(
    importObject.properties.flatMap((property) => {
      if (
        ts.isPropertyAssignment(property) ||
        ts.isMethodDeclaration(property) ||
        ts.isShorthandPropertyAssignment(property)
      ) {
        return [propertyText(property.name, file)];
      }
      return [];
    })
  );
}
