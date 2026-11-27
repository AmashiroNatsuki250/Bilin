import { definePreset } from '@primeuix/themes';
import Aura from '@primeuix/themes/aura';

const neutral = {
    0: '#fff', 50: '#fafafa', 100: '#f3f3f3', 200: '#e6e6e6', 300: '#d4d4d4',
    400: '#a5a5a5', 500: '#747474', 600: '#565656', 700: '#404040',
    800: '#292929', 900: '#1d1d1d', 950: '#111',
};

export default definePreset(Aura, {
    semantic: {
        primary: neutral,
        colorScheme: {
            light: {
                surface: neutral,
                primary: { color: '#202020', inverseColor: '#fff', hoverColor: '#3d3d3d', activeColor: '#111' },
                highlight: { background: '#eeeeee', focusBackground: '#e3e3e3', color: '#222', focusColor: '#111' },
            },
        },
    },
});
