import { Command } from 'commander';

import { addExamples, c, exitWithError } from '../output.js';

export function registerCompletionsCommand(program: Command): void {
  const cmd = program
    .command('completions <shell>')
    .description('Generate shell completion script (bash, zsh, fish)')
    .action((shell: string) => {
      const name = 'vendo';

      switch (shell) {
        case 'bash':
          console.log(generateBashCompletions(name, program));
          break;
        case 'zsh':
          console.log(generateZshCompletions(name, program));
          break;
        case 'fish':
          console.log(generateFishCompletions(name, program));
          break;
        default:
          exitWithError(
            `Unknown shell: ${shell}\n${c.dim('Supported: bash, zsh, fish')}`,
          );
      }
    });

  addExamples(cmd, [
    'vendo completions bash',
    'vendo completions zsh',
    'vendo completions fish',
  ]);
}

function getCommandNames(cmd: Command): string[] {
  return cmd.commands.map((c) => c.name());
}

function getSubcommandNames(cmd: Command, parentName: string): string[] {
  const parent = cmd.commands.find((c) => c.name() === parentName);
  return parent ? parent.commands.map((c) => c.name()) : [];
}

function generateBashCompletions(name: string, program: Command): string {
  const topLevel = getCommandNames(program);

  const subcommands: Record<string, string[]> = {};
  for (const cmdName of topLevel) {
    const subs = getSubcommandNames(program, cmdName);
    if (subs.length > 0) {
      subcommands[cmdName] = subs;
    }
  }

  let script = `# ${name} bash completions
# Add to ~/.bashrc: eval "$(${name} completions bash)"

_${name}_completions() {
  local cur prev commands
  cur="\${COMP_WORDS[COMP_CWORD]}"
  prev="\${COMP_WORDS[COMP_CWORD-1]}"
  commands="${topLevel.join(' ')}"

  case "\${prev}" in
`;

  for (const [cmd, subs] of Object.entries(subcommands)) {
    script += `    ${cmd})\n      COMPREPLY=( $(compgen -W "${subs.join(' ')}" -- "\${cur}") )\n      return 0\n      ;;\n`;
  }

  script += `    ${name})
      COMPREPLY=( $(compgen -W "\${commands}" -- "\${cur}") )
      return 0
      ;;
  esac
}

complete -F _${name}_completions ${name}`;

  return script;
}

function generateZshCompletions(name: string, program: Command): string {
  const topLevel = getCommandNames(program);

  const subcommands: Record<string, string[]> = {};
  for (const cmdName of topLevel) {
    const subs = getSubcommandNames(program, cmdName);
    if (subs.length > 0) {
      subcommands[cmdName] = subs;
    }
  }

  let script = `# ${name} zsh completions
# Add to ~/.zshrc: eval "$(${name} completions zsh)"

_${name}() {
  local -a commands subcommands

  commands=(${topLevel.map((c) => `'${c}'`).join(' ')})

  if (( CURRENT == 2 )); then
    _describe 'command' commands
    return
  fi

  case "\${words[2]}" in
`;

  for (const [cmd, subs] of Object.entries(subcommands)) {
    script += `    ${cmd})\n      subcommands=(${subs.map((s) => `'${s}'`).join(' ')})\n      _describe 'subcommand' subcommands\n      ;;\n`;
  }

  script += `  esac
}

compdef _${name} ${name}`;

  return script;
}

function generateFishCompletions(name: string, program: Command): string {
  const topLevel = getCommandNames(program);

  let script = `# ${name} fish completions
# Save to ~/.config/fish/completions/${name}.fish

# Disable file completions
complete -c ${name} -f

# Top-level commands
`;

  for (const cmdName of topLevel) {
    const cmd = program.commands.find((c) => c.name() === cmdName);
    const desc = cmd?.description() ?? '';
    script += `complete -c ${name} -n '__fish_use_subcommand' -a '${cmdName}' -d '${desc}'\n`;
  }

  // Subcommands
  for (const cmdName of topLevel) {
    const subs = getSubcommandNames(program, cmdName);
    if (subs.length > 0) {
      script += `\n# ${cmdName} subcommands\n`;
      const parentCmd = program.commands.find((c) => c.name() === cmdName);
      for (const sub of subs) {
        const subCmd = parentCmd?.commands.find((c) => c.name() === sub);
        const desc = subCmd?.description() ?? '';
        script += `complete -c ${name} -n '__fish_seen_subcommand_from ${cmdName}' -a '${sub}' -d '${desc}'\n`;
      }
    }
  }

  return script;
}
