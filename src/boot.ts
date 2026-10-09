import { satellite } from './panel-context';
import './style.css';
import './workspace.css';
if (satellite) void import('./panel-window');
else void import('./main');
