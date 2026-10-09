import { editCommands, shortcutCommand, type EditCommandId } from './shortcuts';
export const commands=editCommands;
export type CommandId=EditCommandId;
export function keyCommand(event:KeyboardEvent):CommandId|undefined {
 const id=shortcutCommand(event);return commands.some(c=>c[0]===id)?id as CommandId:undefined;
}
