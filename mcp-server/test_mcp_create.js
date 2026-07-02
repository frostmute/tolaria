import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js'

async function run() {
  const transport = new StdioClientTransport({
    command: '/usr/local/bin/node',
    args: ['/Users/thewytchhaus/Documents/GitHub/tolaria/mcp-server/index.js']
  });
  const client = new Client(
    { name: 'test-client', version: '1.0.0' },
    { capabilities: { tools: {} } }
  );
  await client.connect(transport);
  const result = await client.callTool({
    name: 'create_note',
    arguments: { 
      vaultPath: '/Users/thewytchhaus/Documents/GitHub/tolaria',
      title: 'AGENTS',
      content: 'testing',
      frontmatter: {
        type: 'Note'
      }
    }
  });
  console.log(JSON.stringify(result, null, 2));
  process.exit(0);
}
run();
